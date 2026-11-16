import { useState } from "react";
import {
  AlertTriangle,
  CheckCircle2,
  Rocket,
  RotateCcw,
  Save,
  ShieldCheck,
  Tag,
} from "lucide-react";
import type {
  FlowDeploymentRecord,
  FlowIssue,
  FlowVersionRecord,
} from "@/lib/api/axflow";
import type { Environment } from "@/lib/api/enums";
import { Badge, Button, Input } from "@/components/ui/primitives";
import { cn } from "@/lib/utils";

export interface VersionDeployPanelProps {
  isDirty: boolean;
  saving: boolean;
  validating: boolean;
  publishing: boolean;
  deployingEnv: Environment | null;
  issues: FlowIssue[];
  versions: FlowVersionRecord[];
  deployments: FlowDeploymentRecord[];
  onSaveDraft: () => void;
  onValidateDraft: () => void;
  onPublishVersion: (note: string) => Promise<void>;
  onDeployVersion: (environment: Environment, version: number) => Promise<void>;
  onFocusNode?: (nodeId: string) => void;
}

export function VersionDeployPanel({
  isDirty,
  saving,
  validating,
  publishing,
  deployingEnv,
  issues,
  versions,
  deployments,
  onSaveDraft,
  onValidateDraft,
  onPublishVersion,
  onDeployVersion,
  onFocusNode,
}: VersionDeployPanelProps) {
  const [releaseNote, setReleaseNote] = useState("");

  const testDep =
    deployments.find((d) => d.environment === "TEST") ?? null;
  const prodDep =
    deployments.find((d) => d.environment === "PROD") ?? null;
  const errorCount = issues.filter((i) => i.severity === "ERROR").length;

  const handlePublish = async () => {
    await onPublishVersion(releaseNote.trim());
    setReleaseNote("");
  };

  return (
    <div
      className="space-y-3.5 text-xs"
      data-testid="axflow-version-deploy-panel"
    >
      {/* Draft Actions */}
      <div className="space-y-2">
        <div className="flex items-center justify-between">
          <span className="font-semibold">草稿校验与保存</span>
          {isDirty && <Badge variant="xblue">未保存修改</Badge>}
        </div>
        <div className="grid grid-cols-2 gap-2">
          <Button
            type="button"
            variant="outline"
            className="h-8 text-xs"
            data-testid="validate-flow-btn"
            disabled={validating}
            onClick={onValidateDraft}
          >
            <ShieldCheck className="h-3.5 w-3.5 text-xblue" />
            {validating ? "校验中..." : "静态校验"}
          </Button>
          <Button
            type="button"
            className="h-8 text-xs"
            data-testid="save-flow-btn"
            disabled={saving}
            onClick={onSaveDraft}
          >
            <Save className="h-3.5 w-3.5" />
            {saving ? "保存中..." : "保存草稿"}
          </Button>
        </div>
      </div>

      {/* Validation Diagnostics */}
      <div
        className="rounded-md border border-border bg-muted/20 p-2.5 space-y-1.5"
        data-testid="flow-validation-issues"
      >
        {issues.length === 0 ? (
          <div className="flex items-center gap-1.5 text-emerald-700 dark:text-emerald-400">
            <CheckCircle2 className="h-3.5 w-3.5 shrink-0" />
            <span>DAG 拓扑、参数模板与触发器静态校验通过</span>
          </div>
        ) : (
          <div className="space-y-1.5">
            <div className="flex items-center justify-between font-medium">
              <span className="inline-flex items-center gap-1 text-red-600 dark:text-red-400">
                <AlertTriangle className="h-3.5 w-3.5" />
                发现 {issues.length} 项校验诊断
              </span>
              <Badge variant="outline">错误 {errorCount}</Badge>
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
                    iss.node_id && onFocusNode && "cursor-pointer hover:opacity-90"
                  )}
                >
                  <div className="flex items-center justify-between gap-1 font-mono-tech text-[10px]">
                    <span>{iss.code}</span>
                    {iss.node_id && <span>节点: {iss.node_id}</span>}
                  </div>
                  <div className="mt-0.5">{iss.message}</div>
                </div>
              ))}
            </div>
          </div>
        )}
      </div>

      {/* Publish New Version */}
      <div className="space-y-2 pt-1 border-t border-border">
        <div className="font-semibold">发布不可变版本</div>
        <div className="flex gap-1.5">
          <Input
            data-testid="publish-note-input"
            value={releaseNote}
            onChange={(e) => setReleaseNote(e.target.value)}
            placeholder="版本说明（如：增加补货人工审批门禁）"
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
            {publishing ? "发布中..." : "发布新版本"}
          </Button>
        </div>
      </div>

      {/* Active Environment Deployments */}
      <div className="grid grid-cols-2 gap-2 pt-1 border-t border-border">
        <div
          className="rounded-md border border-border bg-muted/20 p-2.5 space-y-1"
          data-testid="deployment-card-test"
        >
          <div className="text-[10px] font-mono-tech text-muted-foreground">
            TEST 测试环境
          </div>
          <div className="font-mono-tech text-sm font-semibold">
            {testDep ? `v${testDep.version}` : "未部署"}
          </div>
        </div>
        <div
          className="rounded-md border border-xblue/30 bg-xblue-soft/60 p-2.5 space-y-1"
          data-testid="deployment-card-prod"
        >
          <div className="text-[10px] font-mono-tech text-xblue">
            PROD 正式环境
          </div>
          <div className="font-mono-tech text-sm font-semibold text-xblue">
            {prodDep ? `v${prodDep.version}` : "未部署"}
          </div>
        </div>
      </div>

      {/* Version History & Deploy / Rollback */}
      <div className="space-y-2 pt-1 border-t border-border">
        <div className="flex items-center justify-between">
          <span className="font-semibold">已发布版本历史</span>
          <Badge variant="neutral">{versions.length}</Badge>
        </div>

        {versions.length === 0 ? (
          <div className="py-4 text-center text-[11px] text-muted-foreground">
            暂无已发布版本，保存并校验无误后可发布 v1
          </div>
        ) : (
          <div className="max-h-56 overflow-y-auto space-y-2">
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
                      {ver.nodes.length} 节点 · {ver.edges.length} 边
                    </span>
                  </div>

                  {ver.note && (
                    <div className="text-[11px] text-muted-foreground">
                      {ver.note}
                    </div>
                  )}

                  <div className="flex items-center gap-1.5">
                    <Button
                      type="button"
                      variant="outline"
                      className="h-6 px-2 py-0 text-[11px] flex-1"
                      data-testid={`deploy-test-v${ver.version}`}
                      disabled={isTest || deployingEnv !== null}
                      onClick={() => void onDeployVersion("TEST", ver.version)}
                    >
                      <Rocket className="h-3 w-3" />
                      {isTest ? "TEST 运行中" : "部署 TEST"}
                    </Button>
                    <Button
                      type="button"
                      variant="outline"
                      className="h-6 px-2 py-0 text-[11px] flex-1"
                      data-testid={`deploy-prod-v${ver.version}`}
                      disabled={isProd || deployingEnv !== null}
                      onClick={() => void onDeployVersion("PROD", ver.version)}
                    >
                      {isRollbackForProd ? (
                        <>
                          <RotateCcw className="h-3 w-3 text-xblue" />
                          回退 PROD
                        </>
                      ) : (
                        <>
                          <Rocket className="h-3 w-3 text-xblue" />
                          {isProd ? "PROD 运行中" : "部署 PROD"}
                        </>
                      )}
                    </Button>
                  </div>
                </div>
              );
            })}
          </div>
        )}
      </div>
    </div>
  );
}
