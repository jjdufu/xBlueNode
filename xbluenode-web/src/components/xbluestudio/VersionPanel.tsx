import { useEffect, useState } from "react";
import { GitCompare, History, RotateCcw, X } from "lucide-react";
import {
  xblueApi,
  type SchemaDiff,
  type SchemaVersionSummary,
} from "@/lib/api/xblue";
import { Badge, Button } from "@/components/ui/primitives";
import { DiffView } from "./DiffView";

export interface VersionPanelProps {
  open: boolean;
  subgraphSlug: string;
  versions: SchemaVersionSummary[];
  onClose: () => void;
  onRestoreVersion: (version: number) => Promise<void>;
}

function formatTime(iso: string): string {
  if (!iso) return "--";
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  return d.toLocaleString("zh-CN", { hour12: false });
}

export function VersionPanel({
  open,
  subgraphSlug,
  versions,
  onClose,
  onRestoreVersion,
}: VersionPanelProps) {
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
  }, [open, subgraphSlug, versions.length]);

  if (!open) return null;

  const runDiff = async () => {
    setDiffLoading(true);
    setMsg(null);
    try {
      const d = await xblueApi.diffSchemaVersions(verA, verB, subgraphSlug);
      setDiffResult(d);
    } catch (err) {
      setMsg(err instanceof Error ? err.message : "加载版本差异失败");
    } finally {
      setDiffLoading(false);
    }
  };

  const handleRestore = async (v: number) => {
    setRestoringVer(v);
    setMsg(null);
    try {
      await onRestoreVersion(v);
      onClose();
    } catch (err) {
      setMsg(err instanceof Error ? err.message : "恢复草案失败");
    } finally {
      setRestoringVer(null);
    }
  };

  return (
    <div
      className="fixed inset-0 z-50 flex justify-end bg-black/40"
      data-testid="version-panel"
    >
      <div className="h-full w-full max-w-xl bg-card border-l border-border shadow-xl flex flex-col">
        <div className="flex items-center justify-between border-b border-border px-4 py-3">
          <div className="flex items-center gap-2">
            <History className="h-4 w-4 text-xblue" />
            <div>
              <h2 className="text-sm font-semibold">
                模型版本历史与差异回滚 · {subgraphSlug}
              </h2>
              <p className="text-[11px] text-muted-foreground">
                每次发布产生不可变快照，支持任意两版本对比或一键恢复为草案
              </p>
            </div>
          </div>
          <button
            type="button"
            data-testid="close-version-panel-btn"
            onClick={onClose}
            className="rounded p-1 hover:bg-muted text-muted-foreground cursor-pointer"
          >
            <X className="h-4 w-4" />
          </button>
        </div>

        <div className="flex-1 overflow-y-auto p-4 space-y-4 text-xs">
          {msg && (
            <div className="rounded border border-border bg-muted p-2 text-xs">
              {msg}
            </div>
          )}

          {/* Version History List */}
          <div className="space-y-2">
            <div className="font-semibold">
              已发布版本列表 ({sorted.length})
            </div>
            {sorted.length === 0 ? (
              <div className="rounded border border-dashed border-border p-4 text-center text-muted-foreground">
                当前子图尚无历史发布记录。
              </div>
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
                          {item.change_summary || "基线模型快照"}
                        </span>
                      </div>
                      <div className="font-mono-tech text-[11px] text-muted-foreground">
                        {formatTime(item.published_at)}
                      </div>
                    </div>
                    <Button
                      variant="outline"
                      className="h-7 px-2.5 py-1 text-xs shrink-0"
                      data-testid={`restore-version-v${item.version}`}
                      disabled={restoringVer !== null}
                      onClick={() => void handleRestore(item.version)}
                    >
                      <RotateCcw className="h-3 w-3" />
                      {restoringVer === item.version
                        ? "生成中..."
                        : "恢复为草案"}
                    </Button>
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
                  版本结构差异对比
                </span>
                <div className="flex items-center gap-1.5">
                  <select
                    data-testid="diff-from-version"
                    value={verA}
                    onChange={(e) => setVerA(Number(e.target.value))}
                    className="h-7 rounded border border-input bg-background px-2 font-mono-tech text-xs"
                  >
                    {sorted.map((v) => (
                      <option key={`a-${v.version}`} value={v.version}>
                        基准 v{v.version}
                      </option>
                    ))}
                  </select>
                  <span className="text-muted-foreground">→</span>
                  <select
                    data-testid="diff-to-version"
                    value={verB}
                    onChange={(e) => setVerB(Number(e.target.value))}
                    className="h-7 rounded border border-input bg-background px-2 font-mono-tech text-xs"
                  >
                    {sorted.map((v) => (
                      <option key={`b-${v.version}`} value={v.version}>
                        目标 v{v.version}
                      </option>
                    ))}
                  </select>
                  <Button
                    variant="outline"
                    className="h-7 px-2.5 py-1 text-xs"
                    data-testid="run-version-diff-btn"
                    disabled={diffLoading}
                    onClick={() => void runDiff()}
                  >
                    {diffLoading ? "对比中..." : "对比差异"}
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
        </div>
      </div>
    </div>
  );
}
