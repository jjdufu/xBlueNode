import { useMemo, useState } from "react";
import {
  ArrowUpRight,
  Check,
  Copy,
  FileJson,
  Plus,
  Trash2,
  X,
} from "lucide-react";
import type { EndpointKind, Environment } from "@/lib/api/enums";
import {
  deleteEndpoint,
  fetchEndpointSkillManifest,
  fetchPublishedOpenApi,
  promoteEndpoint,
  updateEndpoint,
  type AgentRecord,
  type EndpointSpec,
  type SkillManifest,
} from "@/lib/api/axagent";
import { Badge, Button, Card } from "@/components/ui/primitives";
import { cn } from "@/lib/utils";
import { EndpointPublishDialog } from "./EndpointPublishDialog";

export interface EndpointListProps {
  endpoints: EndpointSpec[];
  agents: AgentRecord[];
  metricNames?: string[];
  objectTypeNames?: string[];
  onRefresh: () => Promise<void>;
}

export function EndpointList({
  endpoints,
  agents,
  metricNames = [],
  objectTypeNames = [],
  onRefresh,
}: EndpointListProps) {
  const [envFilter, setEnvFilter] = useState<"ALL" | Environment>("ALL");
  const [kindFilter, setKindFilter] = useState<"ALL" | EndpointKind>("ALL");
  const [publishOpen, setPublishOpen] = useState(false);

  const [busyId, setBusyId] = useState<string | null>(null);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);
  const [feedbackMsg, setFeedbackMsg] = useState<string | null>(null);

  const [skillModal, setSkillModal] = useState<{
    endpoint: EndpointSpec;
    manifest: SkillManifest;
  } | null>(null);
  const [openApiModal, setOpenApiModal] = useState<{
    env: Environment;
    doc: Record<string, unknown>;
  } | null>(null);
  const [copiedId, setCopiedId] = useState<string | null>(null);

  const filteredEndpoints = useMemo(
    () =>
      endpoints.filter((ep) => {
        if (envFilter !== "ALL" && ep.environment !== envFilter) return false;
        if (kindFilter !== "ALL" && ep.kind !== kindFilter) return false;
        return true;
      }),
    [endpoints, envFilter, kindFilter]
  );

  const handleToggleFlag = async (
    ep: EndpointSpec,
    field: "enabled" | "expose_mcp" | "show_as_app"
  ) => {
    setBusyId(ep.id);
    setErrorMsg(null);
    try {
      await updateEndpoint(ep.id, { [field]: !ep[field] });
      await onRefresh();
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : "更新端点配置失败");
    } finally {
      setBusyId(null);
    }
  };

  const handlePromote = async (ep: EndpointSpec) => {
    setBusyId(ep.id);
    setErrorMsg(null);
    setFeedbackMsg(null);
    try {
      const prodEp = await promoteEndpoint(ep.id);
      await onRefresh();
      setFeedbackMsg(
        `已将端点 ${prodEp.slug} 提升至 PROD 环境${
          prodEp.target_version !== null
            ? `（锁定版本 v${prodEp.target_version}）`
            : ""
        }`
      );
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : "提升至 PROD 失败");
    } finally {
      setBusyId(null);
    }
  };

  const handleDelete = async (ep: EndpointSpec) => {
    setBusyId(ep.id);
    setErrorMsg(null);
    try {
      await deleteEndpoint(ep.id);
      await onRefresh();
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : "删除端点失败");
    } finally {
      setBusyId(null);
    }
  };

  const handleOpenSkill = async (ep: EndpointSpec) => {
    setBusyId(ep.id);
    setErrorMsg(null);
    try {
      const manifest = await fetchEndpointSkillManifest(ep.id);
      setSkillModal({ endpoint: ep, manifest });
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : "读取 Skill 清单失败");
    } finally {
      setBusyId(null);
    }
  };

  const handleOpenApi = async (env: Environment = "PROD") => {
    setErrorMsg(null);
    try {
      const doc = await fetchPublishedOpenApi(env);
      setOpenApiModal({ env, doc });
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : "导出 OpenAPI 3.1 失败");
    }
  };

  const copyCurl = async (ep: EndpointSpec) => {
    const origin =
      typeof window !== "undefined" ? window.location.origin : "http://127.0.0.1:9995";
    const cmd = `curl -X POST "${origin}/api/published/${ep.environment.toLowerCase()}/${ep.tenant_id}/${ep.slug}" -H "Authorization: Bearer axp_<PAT>" -H "Content-Type: application/json" -d '{}'`;
    try {
      await navigator.clipboard.writeText(cmd);
      setCopiedId(ep.id);
      setTimeout(() => setCopiedId(null), 1600);
    } catch {
      // Ignore clipboard failure in headless browsers.
    }
  };

  return (
    <div className="space-y-4">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div className="flex items-center flex-wrap gap-2">
          <div className="inline-flex rounded-md border border-border bg-muted/40 p-0.5 text-xs">
            {(["ALL", "TEST", "PROD"] as const).map((env) => (
              <button
                key={env}
                type="button"
                data-testid={`endpoint-env-filter-${env}`}
                onClick={() => setEnvFilter(env)}
                className={cn(
                  "rounded px-2.5 py-1 text-xs font-medium transition-colors cursor-pointer",
                  envFilter === env
                    ? "bg-background text-foreground shadow-xs"
                    : "text-muted-foreground hover:text-foreground"
                )}
              >
                {env === "ALL" ? "全部环境" : env}
              </button>
            ))}
          </div>

          <select
            data-testid="endpoint-kind-filter"
            value={kindFilter}
            onChange={(e) =>
              setKindFilter(e.target.value as "ALL" | EndpointKind)
            }
            className="h-8 rounded-md border border-input bg-background px-2.5 text-xs font-mono-tech"
          >
            <option value="ALL">全部类型</option>
            <option value="METRIC">METRIC</option>
            <option value="SAVED_QUERY">SAVED_QUERY</option>
            <option value="OBJECT_TYPE">OBJECT_TYPE</option>
            <option value="FLOW">FLOW</option>
            <option value="AGENT">AGENT</option>
          </select>
        </div>

        <div className="flex items-center gap-2">
          <Button
            type="button"
            variant="outline"
            data-testid="view-openapi-btn"
            onClick={() => void handleOpenApi("PROD")}
          >
            <FileJson className="h-3.5 w-3.5" />
            OpenAPI 3.1
          </Button>
          <Button
            type="button"
            data-testid="open-publish-endpoint-btn"
            onClick={() => setPublishOpen(true)}
          >
            <Plus className="h-3.5 w-3.5" />
            发布新端点
          </Button>
        </div>
      </div>

      {errorMsg ? (
        <div className="rounded-md border border-border bg-muted px-3 py-2 text-xs text-foreground">
          {errorMsg}
        </div>
      ) : null}
      {feedbackMsg ? (
        <div
          data-testid="endpoint-promote-feedback"
          className="rounded-md border border-xblue/40 bg-xblue-soft/40 px-3 py-2 text-xs text-foreground"
        >
          {feedbackMsg}
        </div>
      ) : null}

      <Card className="overflow-hidden">
        {filteredEndpoints.length === 0 ? (
          <div className="p-8 text-center text-xs text-muted-foreground">
            当前筛选条件下暂无已发布能力端点
          </div>
        ) : (
          <div className="overflow-x-auto">
            <table className="w-full text-left text-xs">
              <thead className="bg-muted/50 text-muted-foreground border-b border-border">
                <tr>
                  <th className="px-4 py-2.5 font-medium">端点名称 / Slug</th>
                  <th className="px-3 py-2.5 font-medium">环境与版本</th>
                  <th className="px-3 py-2.5 font-medium">能力类型</th>
                  <th className="px-3 py-2.5 font-medium">限流 / 缓存</th>
                  <th className="px-3 py-2.5 font-medium">开关策略</th>
                  <th className="px-4 py-2.5 font-medium text-right">操作</th>
                </tr>
              </thead>
              <tbody className="divide-y divide-border">
                {filteredEndpoints.map((ep) => (
                  <tr
                    key={ep.id}
                    data-testid={`endpoint-row-${ep.slug}-${ep.environment}`}
                    className="hover:bg-muted/20"
                  >
                    <td className="px-4 py-3 space-y-0.5">
                      <div className="font-semibold text-foreground">
                        {ep.title}
                      </div>
                      <div className="font-mono-tech text-[11px] text-muted-foreground">
                        /api/published/{ep.environment.toLowerCase()}/
                        {ep.tenant_id}/{ep.slug}
                      </div>
                    </td>
                    <td className="px-3 py-3">
                      <div className="flex items-center gap-1.5">
                        <Badge
                          variant={
                            ep.environment === "PROD" ? "xblue" : "neutral"
                          }
                        >
                          {ep.environment}
                        </Badge>
                        <Badge variant="outline">
                          {ep.target_version !== null
                            ? `固定 v${ep.target_version}`
                            : "跟随最新"}
                        </Badge>
                      </div>
                    </td>
                    <td className="px-3 py-3">
                      <div className="space-y-0.5">
                        <Badge variant="outline">{ep.kind}</Badge>
                        <div className="font-mono-tech text-[11px] text-muted-foreground truncate max-w-[140px]">
                          {ep.target_id}
                        </div>
                      </div>
                    </td>
                    <td className="px-3 py-3 font-mono-tech text-[11px] text-muted-foreground">
                      <div>{ep.rate_limit_rpm} RPM</div>
                      <div>TTL {ep.cache_ttl_s}s</div>
                    </td>
                    <td className="px-3 py-3">
                      <div className="flex flex-wrap gap-1.5">
                        <button
                          type="button"
                          disabled={busyId === ep.id}
                          onClick={() => void handleToggleFlag(ep, "enabled")}
                          className={cn(
                            "rounded border px-1.5 py-0.5 text-[10px] font-mono-tech cursor-pointer",
                            ep.enabled
                              ? "border-xblue bg-xblue-soft text-xblue"
                              : "border-border text-muted-foreground"
                          )}
                        >
                          {ep.enabled ? "启用" : "停用"}
                        </button>
                        <button
                          type="button"
                          disabled={busyId === ep.id}
                          onClick={() =>
                            void handleToggleFlag(ep, "expose_mcp")
                          }
                          className={cn(
                            "rounded border px-1.5 py-0.5 text-[10px] font-mono-tech cursor-pointer",
                            ep.expose_mcp
                              ? "border-xblue bg-xblue-soft text-xblue"
                              : "border-border text-muted-foreground"
                          )}
                        >
                          MCP
                        </button>
                        <button
                          type="button"
                          disabled={busyId === ep.id}
                          onClick={() =>
                            void handleToggleFlag(ep, "show_as_app")
                          }
                          className={cn(
                            "rounded border px-1.5 py-0.5 text-[10px] font-mono-tech cursor-pointer",
                            ep.show_as_app
                              ? "border-xblue bg-xblue-soft text-xblue"
                              : "border-border text-muted-foreground"
                          )}
                        >
                          App
                        </button>
                      </div>
                    </td>
                    <td className="px-4 py-3">
                      <div className="flex items-center justify-end gap-1.5">
                        {ep.environment === "TEST" ? (
                          <Button
                            type="button"
                            variant="outline"
                            data-testid={`promote-endpoint-btn-${ep.slug}`}
                            disabled={busyId === ep.id}
                            onClick={() => void handlePromote(ep)}
                            className="h-7 px-2.5"
                          >
                            <ArrowUpRight className="h-3.5 w-3.5 text-xblue" />
                            提升至 PROD
                          </Button>
                        ) : null}
                        <Button
                          type="button"
                          variant="outline"
                          data-testid={`skill-manifest-btn-${ep.slug}-${ep.environment}`}
                          disabled={busyId === ep.id}
                          onClick={() => void handleOpenSkill(ep)}
                          className="h-7 px-2.5"
                        >
                          Skill JSON
                        </Button>
                        <Button
                          type="button"
                          variant="ghost"
                          onClick={() => void copyCurl(ep)}
                          className="h-7 px-2"
                        >
                          {copiedId === ep.id ? (
                            <Check className="h-3.5 w-3.5 text-xblue" />
                          ) : (
                            <Copy className="h-3.5 w-3.5" />
                          )}
                        </Button>
                        <Button
                          type="button"
                          variant="ghost"
                          disabled={busyId === ep.id}
                          onClick={() => void handleDelete(ep)}
                          className="h-7 px-2"
                        >
                          <Trash2 className="h-3.5 w-3.5" />
                        </Button>
                      </div>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </Card>

      <EndpointPublishDialog
        open={publishOpen}
        agents={agents}
        metricNames={metricNames}
        objectTypeNames={objectTypeNames}
        onClose={() => setPublishOpen(false)}
        onCreated={async () => {
          await onRefresh();
        }}
      />

      {/* Skill Manifest Modal */}
      {skillModal ? (
        <div
          data-testid="skill-manifest-modal"
          className="fixed inset-0 z-50 flex items-center justify-center bg-black/50 p-4"
        >
          <Card className="w-full max-w-xl p-5 space-y-3">
            <div className="flex items-center justify-between border-b border-border pb-2.5">
              <h3 className="text-sm font-semibold">
                Skill 清单 · {skillModal.endpoint.slug}
              </h3>
              <Button
                type="button"
                variant="ghost"
                onClick={() => setSkillModal(null)}
                className="h-7 w-7 p-0"
              >
                <X className="h-4 w-4" />
              </Button>
            </div>
            <pre className="rounded-md border border-border bg-muted/40 p-3 font-mono-tech text-[11px] overflow-x-auto max-h-80">
              {JSON.stringify(skillModal.manifest, null, 2)}
            </pre>
          </Card>
        </div>
      ) : null}

      {/* OpenAPI 3.1 Modal */}
      {openApiModal ? (
        <div
          data-testid="openapi-modal"
          className="fixed inset-0 z-50 flex items-center justify-center bg-black/50 p-4"
        >
          <Card className="w-full max-w-2xl p-5 space-y-3">
            <div className="flex items-center justify-between border-b border-border pb-2.5">
              <div className="flex items-center gap-2">
                <h3 className="text-sm font-semibold">
                  OpenAPI 3.1 规范文档 ({openApiModal.env})
                </h3>
                <Button
                  type="button"
                  variant="outline"
                  onClick={() =>
                    void handleOpenApi(
                      openApiModal.env === "PROD" ? "TEST" : "PROD"
                    )
                  }
                  className="h-6 px-2 text-[11px]"
                >
                  切换至 {openApiModal.env === "PROD" ? "TEST" : "PROD"}
                </Button>
              </div>
              <Button
                type="button"
                variant="ghost"
                data-testid="close-openapi-modal-btn"
                onClick={() => setOpenApiModal(null)}
                className="h-7 w-7 p-0"
              >
                <X className="h-4 w-4" />
              </Button>
            </div>
            <pre className="rounded-md border border-border bg-muted/40 p-3 font-mono-tech text-[11px] overflow-x-auto max-h-96">
              {JSON.stringify(openApiModal.doc, null, 2)}
            </pre>
          </Card>
        </div>
      ) : null}
    </div>
  );
}
