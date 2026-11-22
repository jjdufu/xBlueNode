import { useEffect, useState } from "react";
import { GitCompare, History, RotateCcw } from "lucide-react";
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
import { DiffView } from "./DiffView";

export interface VersionPanelProps {
  open: boolean;
  subgraphSlug: string;
  versions: SchemaVersionSummary[];
  onClose: () => void;
  onRestoreVersion: (version: number) => Promise<void>;
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

  const runDiff = async () => {
    setDiffLoading(true);
    setMsg(null);
    try {
      const d = await xblueApi.diffSchemaVersions(verA, verB, subgraphSlug);
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
    if (!canEdit) return;
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
            {tx("版本历史与差异回滚", "Version History & Diff")} · {subgraphSlug}
          </span>
        </>
      }
    >
      {msg && (
        <div className="rounded border border-border bg-muted p-2 text-xs">
          {msg}
        </div>
      )}

      {/* Version History List */}
      <div className="space-y-2">
        <div className="font-semibold">
          {tx("已发布版本", "Published Versions")} ({sorted.length})
        </div>
        {sorted.length === 0 ? (
          <EmptyState title={tx("暂无发布版本", "No published versions")} />
        ) : (
          <div className="space-y-1.5" data-testid="version-list">
            {sorted.map((item, idx) => (
              <div
                key={`${item.version}-${idx}`}
                data-testid={`version-item-v${item.version}`}
                className="flex items-center justify-between gap-2 rounded border border-border bg-muted/20 px-3 py-2"
              >
                <div className="min-w-0 space-y-0.5">
                  <div className="flex items-center gap-2">
                    <Badge variant={idx === 0 ? "xblue" : "neutral"}>
                      v{item.version}
                    </Badge>
                    <span className="font-medium truncate">
                      {item.change_summary ||
                        tx("基线模型快照", "Baseline Snapshot")}
                    </span>
                  </div>
                  <div className="font-mono-tech text-[11px] text-muted-foreground">
                    {fmtTime(item.published_at)}
                  </div>
                </div>
                {canEdit && (
                  <Button
                    variant="outline"
                    size="sm"
                    data-testid={`restore-version-v${item.version}`}
                    disabled={restoringVer !== null}
                    onClick={() => void handleRestore(item.version)}
                  >
                    <RotateCcw className="h-3 w-3" />
                    {restoringVer === item.version
                      ? tx("恢复中…", "Restoring…")
                      : tx("恢复草案", "Restore")}
                  </Button>
                )}
              </div>
            ))}
          </div>
        )}
      </div>

      {/* Arbitrary Two-Version Comparison */}
      {sorted.length >= 1 && (
        <div className="space-y-2.5 border-t border-border pt-3">
          <div className="flex flex-wrap items-center justify-between gap-2">
            <span className="font-semibold flex items-center gap-1.5">
              <GitCompare className="h-3.5 w-3.5 text-xblue" />
              {tx("版本差异对比", "Version Diff")}
            </span>
            <div className="flex items-center gap-1.5">
              <Select
                data-testid="diff-from-version"
                value={verA}
                onChange={(e) => setVerA(Number(e.target.value))}
                className="h-7 px-2 font-mono-tech"
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
                className="h-7 px-2 font-mono-tech"
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
    </Drawer>
  );
}
