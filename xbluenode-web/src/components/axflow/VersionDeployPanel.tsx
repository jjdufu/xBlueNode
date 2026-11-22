import { useState } from "react";
import {
  AlertTriangle,
  CheckCircle2,
  Rocket,
  RotateCcw,
  ShieldCheck,
  Tag,
} from "lucide-react";
import { useI18n } from "@/i18n";
import { usePrincipal } from "@/lib/permissions";
import type {
  FlowDeploymentRecord,
  FlowIssue,
  FlowVersionRecord,
} from "@/lib/api/axflow";
import type { Environment } from "@/lib/api/enums";
import { Badge, Button, Input, StatText } from "@/components/ui/primitives";
import { cn } from "@/lib/utils";

export interface VersionDeployPanelProps {
  isDirty: boolean;
  validating: boolean;
  publishing: boolean;
  deployingEnv: Environment | null;
  issues: FlowIssue[];
  versions: FlowVersionRecord[];
  deployments: FlowDeploymentRecord[];
  onValidateDraft: () => void;
  onPublishVersion: (note: string) => Promise<void>;
  onDeployVersion: (
    environment: Environment,
    version: number
  ) => Promise<void>;
  onFocusNode?: (nodeId: string) => void;
}

export function VersionDeployPanel({
  isDirty,
  validating,
  publishing,
  deployingEnv,
  issues,
  versions,
  deployments,
  onValidateDraft,
  onPublishVersion,
  onDeployVersion,
  onFocusNode,
}: VersionDeployPanelProps) {
  const { tx } = useI18n();
  const { can } = usePrincipal();
  const canPublish = can("FLOW_PUBLISH");

  const [releaseNote, setReleaseNote] = useState("");

  const testDep = deployments.find((d) => d.environment === "TEST") ?? null;
  const prodDep = deployments.find((d) => d.environment === "PROD") ?? null;
  const errorCount = issues.filter((i) => i.severity === "ERROR").length;

  const handlePublish = async () => {
    if (!canPublish) return;
    await onPublishVersion(releaseNote.trim());
    setReleaseNote("");
  };

  return (
    <div
      className="space-y-3.5 text-xs"
      data-testid="axflow-version-deploy-panel"
    >
      {/* Static Validation Action (Single Save Draft lives in Top Bar) */}
      <div className="flex items-center justify-between gap-2">
        <div className="flex items-center gap-1.5">
          <span className="font-semibold">
            {tx("静态拓扑校验", "Static Validation")}
          </span>
          {isDirty && (
            <Badge variant="warning">{tx("未保存", "Unsaved")}</Badge>
          )}
        </div>
        <Button
          type="button"
          variant="outline"
          size="sm"
          data-testid="validate-flow-btn"
          disabled={validating}
          onClick={onValidateDraft}
        >
          <ShieldCheck className="h-3.5 w-3.5 text-xblue" />
          {validating
            ? tx("校验中…", "Validating…")
            : tx("静态校验", "Validate")}
        </Button>
      </div>

      {/* Validation Diagnostics */}
      <div
        className="rounded-md border border-border bg-muted/20 p-2.5 space-y-1.5"
        data-testid="flow-validation-issues"
      >
        {issues.length === 0 ? (
          <div className="flex items-center gap-1.5 text-emerald-700 dark:text-emerald-400">
            <CheckCircle2 className="h-3.5 w-3.5 shrink-0" />
            <span>
              {tx("拓扑与参数静态校验通过", "Static DAG validation passed")}
            </span>
          </div>
        ) : (
          <div className="space-y-1.5">
            <div className="flex items-center justify-between font-medium">
              <span className="inline-flex items-center gap-1 text-red-600 dark:text-red-400">
                <AlertTriangle className="h-3.5 w-3.5" />
                {tx(`${issues.length} 项校验诊断`, `${issues.length} Issues`)}
              </span>
              <Badge variant="danger">
                {tx(`错误 ${errorCount}`, `${errorCount} Errors`)}
              </Badge>
            </div>
            <div className="max-h-32 overflow-y-auto space-y-1">
              {issues.map((iss, idx) => (
                <div
                  key={`${iss.code}-${iss.node_id ?? "root"}-${idx}`}
                  onClick={() => {
                    if (iss.node_id && onFocusNode) {
                      onFocusNode(iss.node_id);
                    }
                  }}
                  className={cn(
                    "rounded border p-1.5 text-[11px]",
                    iss.severity === "ERROR"
                      ? "border-red-500/30 bg-red-500/5 text-red-700 dark:text-red-300"
                      : "border-amber-500/30 bg-amber-500/5 text-amber-700 dark:text-amber-300",
                    iss.node_id &&
                      onFocusNode &&
                      "cursor-pointer hover:opacity-90"
                  )}
                >
                  <div className="flex items-center justify-between gap-1 font-mono-tech text-[10px]">
                    <span>{iss.code}</span>
                    {iss.node_id && (
                      <span>
                        {tx("节点:", "Node:")} {iss.node_id}
                      </span>
                    )}
                  </div>
                  <div className="mt-0.5">{iss.message}</div>
                </div>
              ))}
            </div>
          </div>
        )}
      </div>

      {/* Publish New Version */}
      {canPublish && (
        <div className="space-y-2 pt-2.5 border-t border-border">
          <div className="font-semibold">
            {tx("发布不可变版本", "Publish Version")}
          </div>
          <div className="flex gap-1.5">
            <Input
              data-testid="publish-note-input"
              value={releaseNote}
              onChange={(e) => setReleaseNote(e.target.value)}
              placeholder={tx("新增外部回调节点", "Release notes")}
              className="h-8 text-xs"
            />
            <Button
              type="button"
              className="h-8 px-3 shrink-0 text-xs"
              data-testid="publish-version-btn"
              disabled={publishing || errorCount > 0}
              onClick={() => void handlePublish()}
            >
              <Tag className="h-3.5 w-3.5" />
              {publishing
                ? tx("发布中…", "Publishing…")
                : tx("发布版本", "Publish")}
            </Button>
          </div>
        </div>
      )}

      {/* Version History & Deploy / Rollback (Deduplicated — env pills in top bar & per-version) */}
      <div className="space-y-2 pt-2.5 border-t border-border">
        <div className="flex items-center justify-between">
          <span className="font-semibold">
            {tx("版本发布记录", "Version History")}
          </span>
          <StatText label={tx("版本", "Versions")} value={versions.length} />
        </div>

        {versions.length === 0 ? (
          <div className="py-4 text-center text-[11px] text-muted-foreground">
            {tx("暂无已发布版本", "No published versions")}
          </div>
        ) : (
          <div className="space-y-2">
            {versions.map((ver) => {
              const isTest = testDep?.version === ver.version;
              const isProd = prodDep?.version === ver.version;
              const isRollbackForProd =
                prodDep !== null && ver.version < prodDep.version;

              return (
                <div
                  key={ver.id || `v-${ver.version}`}
                  data-testid={`version-item-v${ver.version}`}
                  className="rounded-md border border-border bg-background p-2.5 space-y-2"
                >
                  <div className="flex items-center justify-between gap-1.5">
                    <div className="flex items-center gap-1.5">
                      <span className="font-mono-tech font-semibold text-xs">
                        v{ver.version}
                      </span>
                      {isTest && <Badge variant="neutral">TEST</Badge>}
                      {isProd && <Badge variant="xblue">PROD</Badge>}
                    </div>
                    <span className="font-mono-tech text-[10px] text-muted-foreground">
                      {ver.nodes.length} {tx("节点", "nodes")} ·{" "}
                      {ver.edges.length} {tx("边", "edges")}
                    </span>
                  </div>

                  {ver.note && (
                    <div className="text-[11px] text-muted-foreground">
                      {ver.note}
                    </div>
                  )}

                  {canPublish && (
                    <div className="flex items-center gap-1.5">
                      <Button
                        type="button"
                        variant="outline"
                        size="sm"
                        className="h-7 px-2 text-[11px] flex-1"
                        data-testid={`deploy-test-v${ver.version}`}
                        disabled={isTest || deployingEnv !== null}
                        onClick={() =>
                          void onDeployVersion("TEST", ver.version)
                        }
                      >
                        <Rocket className="h-3 w-3" />
                        {isTest
                          ? tx("TEST 生效", "TEST Active")
                          : tx("部署 TEST", "Deploy TEST")}
                      </Button>
                      <Button
                        type="button"
                        variant="outline"
                        size="sm"
                        className="h-7 px-2 text-[11px] flex-1"
                        data-testid={`deploy-prod-v${ver.version}`}
                        disabled={isProd || deployingEnv !== null}
                        onClick={() =>
                          void onDeployVersion("PROD", ver.version)
                        }
                      >
                        {isRollbackForProd ? (
                          <>
                            <RotateCcw className="h-3 w-3 text-xblue" />
                            {tx("回滚 PROD", "Rollback PROD")}
                          </>
                        ) : (
                          <>
                            <Rocket className="h-3 w-3 text-xblue" />
                            {isProd
                              ? tx("PROD 生效", "PROD Active")
                              : tx("部署 PROD", "Deploy PROD")}
                          </>
                        )}
                      </Button>
                    </div>
                  )}
                </div>
              );
            })}
          </div>
        )}
      </div>
    </div>
  );
}
