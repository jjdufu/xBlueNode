import { useMemo, useState } from "react";
import {
  ArrowUpRight,
  Check,
  Copy,
  FileJson,
  Plus,
  Trash2,
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
import {
  Badge,
  Button,
  Card,
  ConfirmDialog,
  Modal,
  Select,
} from "@/components/ui/primitives";
import { useI18n } from "@/i18n";
import { usePrincipal } from "@/lib/permissions";
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
  const { tx, tEnum } = useI18n();
  const { can } = usePrincipal();
  const canEdit = can("AGENT_EDIT");

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
  const [confirmDeleteEp, setConfirmDeleteEp] = useState<EndpointSpec | null>(
    null
  );

  const copyJson = async (key: string, data: unknown) => {
    try {
      await navigator.clipboard.writeText(JSON.stringify(data, null, 2));
      setCopiedId(key);
      setTimeout(() => setCopiedId(null), 1600);
    } catch {
      // Ignore clipboard failure in headless browsers.
    }
  };

  const copyEndpointUrl = async (ep: EndpointSpec) => {
    const path = `/api/published/${ep.environment.toLowerCase()}/${ep.tenant_id}/${ep.slug}`;
    const origin =
      typeof window !== "undefined" ? window.location.origin : "http://127.0.0.1:9995";
    try {
      await navigator.clipboard.writeText(`${origin}${path}`);
      setCopiedId(`url-${ep.id}`);
      setTimeout(() => setCopiedId(null), 1600);
    } catch {
      // Ignore clipboard failure in headless browsers.
    }
  };

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
    if (!canEdit) return;
    setBusyId(ep.id);
    setErrorMsg(null);
    try {
      await updateEndpoint(ep.id, { [field]: !ep[field] });
      await onRefresh();
    } catch (err) {
      setErrorMsg(
        err instanceof Error
          ? err.message
          : tx("更新端点失败", "Failed to update endpoint")
      );
    } finally {
      setBusyId(null);
    }
  };

  const handlePromote = async (ep: EndpointSpec) => {
    if (!canEdit) return;
    setBusyId(ep.id);
    setErrorMsg(null);
    setFeedbackMsg(null);
    try {
      const prodEp = await promoteEndpoint(ep.id);
      await onRefresh();
      setFeedbackMsg(
        tx(
          `已将端点 ${prodEp.slug} 提升至 PROD 环境`,
          `Promoted endpoint ${prodEp.slug} to PROD`
        )
      );
    } catch (err) {
      setErrorMsg(
        err instanceof Error
          ? err.message
          : tx("提升失败", "Failed to promote endpoint")
      );
    } finally {
      setBusyId(null);
    }
  };

  const handleDelete = async (ep: EndpointSpec) => {
    if (!canEdit) return;
    setBusyId(ep.id);
    setErrorMsg(null);
    try {
      await deleteEndpoint(ep.id);
      setConfirmDeleteEp(null);
      await onRefresh();
    } catch (err) {
      setErrorMsg(
        err instanceof Error
          ? err.message
          : tx("删除失败", "Failed to delete endpoint")
      );
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
      setErrorMsg(
        err instanceof Error
          ? err.message
          : tx("读取 Skill 清单失败", "Failed to load Skill manifest")
      );
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
      setErrorMsg(
        err instanceof Error
          ? err.message
          : tx("读取 OpenAPI 失败", "Failed to fetch OpenAPI specification")
      );
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
                {env === "ALL" ? tx("全部环境", "All Envs") : tEnum(env)}
              </button>
            ))}
          </div>

          <Select
            data-testid="endpoint-kind-filter"
            value={kindFilter}
            onChange={(e) =>
              setKindFilter(e.target.value as "ALL" | EndpointKind)
            }
            className="h-8"
          >
            <option value="ALL">{tx("全部类型", "All Kinds")}</option>
            <option value="METRIC">{tEnum("METRIC")}</option>
            <option value="SAVED_QUERY">{tEnum("SAVED_QUERY")}</option>
            <option value="OBJECT_TYPE">{tEnum("OBJECT_TYPE")}</option>
            <option value="FLOW">{tEnum("FLOW")}</option>
            <option value="AGENT">{tEnum("AGENT")}</option>
          </Select>
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
          {canEdit && (
            <Button
              type="button"
              data-testid="open-publish-endpoint-btn"
              onClick={() => setPublishOpen(true)}
            >
              <Plus className="h-3.5 w-3.5" />
              {tx("发布端点", "Publish Endpoint")}
            </Button>
          )}
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
            {tx("暂无已发布能力端点", "No published endpoints")}
          </div>
        ) : (
          <div className="overflow-x-auto">
            <table className="w-full text-left text-xs">
              <thead className="bg-muted/50 text-muted-foreground border-b border-border">
                <tr>
                  <th className="px-4 py-2.5 font-medium whitespace-nowrap">
                    {tx("端点名称 / 路径", "Endpoint / Path")}
                  </th>
                  <th className="px-3 py-2.5 font-medium whitespace-nowrap">
                    {tx("环境与版本", "Env & Version")}
                  </th>
                  <th className="px-3 py-2.5 font-medium whitespace-nowrap">
                    {tx("能力类型", "Kind")}
                  </th>
                  <th className="px-3 py-2.5 font-medium whitespace-nowrap">
                    {tx("限流 / 缓存", "Rate / Cache")}
                  </th>
                  <th className="px-3 py-2.5 font-medium whitespace-nowrap">
                    {tx("策略", "Flags")}
                  </th>
                  <th className="px-4 py-2.5 font-medium text-right whitespace-nowrap">
                    {tx("操作", "Actions")}
                  </th>
                </tr>
              </thead>
              <tbody className="divide-y divide-border">
                {filteredEndpoints.map((ep) => (
                  <tr
                    key={ep.id}
                    data-testid={`endpoint-row-${ep.slug}-${ep.environment}`}
                    className="hover:bg-muted/20"
                  >
                    <td className="px-4 py-3 space-y-1">
                      <div className="font-semibold text-foreground">
                        {ep.title}
                      </div>
                      <div className="flex items-center gap-1.5 font-mono-tech text-[11px] text-muted-foreground">
                        <Badge variant="outline" className="text-[10px] px-1.5 py-0">
                          POST
                        </Badge>
                        <span className="truncate max-w-[540px]" title={`/api/published/${ep.environment.toLowerCase()}/${ep.tenant_id}/${ep.slug}`}>
                          /api/published/{ep.environment.toLowerCase()}/
                          {ep.tenant_id}/{ep.slug}
                        </span>
                        <button
                          type="button"
                          onClick={() => void copyEndpointUrl(ep)}
                          className="text-muted-foreground hover:text-foreground cursor-pointer p-0.5 rounded"
                          title={tx("复制完整路径", "Copy full URL")}
                          aria-label={tx("复制完整路径", "Copy full URL")}
                        >
                          {copiedId === `url-${ep.id}` ? (
                            <Check className="h-3 w-3 text-xblue" />
                          ) : (
                            <Copy className="h-3 w-3" />
                          )}
                        </button>
                      </div>
                    </td>
                    <td className="px-3 py-3 whitespace-nowrap">
                      <div className="flex items-center gap-1.5">
                        <Badge
                          variant={
                            ep.environment === "PROD" ? "success" : "outline"
                          }
                        >
                          {ep.environment}
                        </Badge>
                        <Badge variant="outline">
                          {ep.target_version !== null
                            ? `v${ep.target_version}`
                            : tx("最新", "Latest")}
                        </Badge>
                      </div>
                    </td>
                    <td className="px-3 py-3 whitespace-nowrap">
                      <div className="space-y-0.5">
                        <Badge variant="outline">{tEnum(ep.kind)}</Badge>
                        <div className="font-mono-tech text-[11px] text-muted-foreground truncate max-w-[140px]">
                          {ep.target_id}
                        </div>
                      </div>
                    </td>
                    <td className="px-3 py-3 font-mono-tech text-[11px] text-muted-foreground whitespace-nowrap">
                      <div>{ep.rate_limit_rpm} RPM</div>
                      <div>TTL {ep.cache_ttl_s}s</div>
                    </td>
                    <td className="px-3 py-3 whitespace-nowrap">
                      <div className="flex flex-wrap gap-1.5">
                        <button
                          type="button"
                          disabled={!canEdit || busyId === ep.id}
                          onClick={() => void handleToggleFlag(ep, "enabled")}
                          className={cn(
                            "rounded border px-1.5 py-0.5 text-[10px] font-mono-tech cursor-pointer disabled:opacity-50",
                            ep.enabled
                              ? "border-xblue bg-xblue-soft text-xblue"
                              : "border-border text-muted-foreground"
                          )}
                        >
                          {ep.enabled
                            ? tx("启用", "Enabled")
                            : tx("停用", "Disabled")}
                        </button>
                        <button
                          type="button"
                          disabled={!canEdit || busyId === ep.id}
                          onClick={() =>
                            void handleToggleFlag(ep, "expose_mcp")
                          }
                          className={cn(
                            "rounded border px-1.5 py-0.5 text-[10px] font-mono-tech cursor-pointer disabled:opacity-50",
                            ep.expose_mcp
                              ? "border-xblue bg-xblue-soft text-xblue"
                              : "border-border text-muted-foreground"
                          )}
                        >
                          MCP
                        </button>
                        <button
                          type="button"
                          disabled={!canEdit || busyId === ep.id}
                          onClick={() =>
                            void handleToggleFlag(ep, "show_as_app")
                          }
                          className={cn(
                            "rounded border px-1.5 py-0.5 text-[10px] font-mono-tech cursor-pointer disabled:opacity-50",
                            ep.show_as_app
                              ? "border-xblue bg-xblue-soft text-xblue"
                              : "border-border text-muted-foreground"
                          )}
                        >
                          App
                        </button>
                      </div>
                    </td>
                    <td className="px-4 py-3 whitespace-nowrap">
                      <div className="flex items-center justify-end gap-1.5">
                        {canEdit && ep.environment === "TEST" ? (
                          <Button
                            type="button"
                            variant="outline"
                            size="sm"
                            data-testid={`promote-endpoint-btn-${ep.slug}`}
                            disabled={busyId === ep.id}
                            onClick={() => void handlePromote(ep)}
                          >
                            <ArrowUpRight className="h-3.5 w-3.5 text-xblue" />
                            {tx("提升至生产", "Promote")}
                          </Button>
                        ) : null}
                        <Button
                          type="button"
                          variant="outline"
                          size="sm"
                          data-testid={`skill-manifest-btn-${ep.slug}-${ep.environment}`}
                          disabled={busyId === ep.id}
                          onClick={() => void handleOpenSkill(ep)}
                        >
                          Skill JSON
                        </Button>
                        <Button
                          type="button"
                          variant="ghost"
                          size="sm"
                          onClick={() => void copyCurl(ep)}
                        >
                          {copiedId === ep.id ? (
                            <Check className="h-3.5 w-3.5 text-xblue" />
                          ) : (
                            <Copy className="h-3.5 w-3.5" />
                          )}
                        </Button>
                        {canEdit && (
                          <div className="pl-1.5 ml-0.5 border-l border-border">
                            <Button
                              type="button"
                              variant="ghost"
                              size="sm"
                              disabled={busyId === ep.id}
                              onClick={() => setConfirmDeleteEp(ep)}
                              className="text-red-600 dark:text-red-400 hover:bg-red-500/10"
                              title={tx("删除端点", "Delete endpoint")}
                            >
                              <Trash2 className="h-3.5 w-3.5" />
                            </Button>
                          </div>
                        )}
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
      <Modal
        open={Boolean(skillModal)}
        onClose={() => setSkillModal(null)}
        maxWidthClass="max-w-2xl"
        testId="skill-manifest-modal"
        title={`Skill JSON · ${skillModal?.endpoint.slug ?? ""}`}
      >
        {skillModal && (
          <div className="space-y-3">
            <div className="flex items-center justify-end">
              <Button
                type="button"
                variant="outline"
                size="sm"
                onClick={() =>
                  void copyJson(`skill-${skillModal.endpoint.id}`, skillModal.manifest)
                }
              >
                {copiedId === `skill-${skillModal.endpoint.id}` ? (
                  <Check className="h-3.5 w-3.5 text-xblue" />
                ) : (
                  <Copy className="h-3.5 w-3.5" />
                )}
                {tx("复制 JSON", "Copy JSON")}
              </Button>
            </div>
            <pre className="rounded-md border border-border bg-muted/40 p-3 font-mono-tech text-[11px] overflow-x-auto max-h-[65vh]">
              {JSON.stringify(skillModal.manifest, null, 2)}
            </pre>
          </div>
        )}
      </Modal>

      {/* OpenAPI 3.1 Modal */}
      <Modal
        open={Boolean(openApiModal)}
        onClose={() => setOpenApiModal(null)}
        maxWidthClass="max-w-2xl"
        testId="openapi-modal"
        closeTestId="close-openapi-modal-btn"
        title={`OpenAPI 3.1 (${openApiModal?.env ?? "PROD"})`}
      >
        {openApiModal && (
          <div className="space-y-3">
            <div className="flex items-center justify-end gap-2">
              <Button
                type="button"
                variant="outline"
                size="sm"
                onClick={() =>
                  void copyJson(`openapi-${openApiModal.env}`, openApiModal.doc)
                }
              >
                {copiedId === `openapi-${openApiModal.env}` ? (
                  <Check className="h-3.5 w-3.5 text-xblue" />
                ) : (
                  <Copy className="h-3.5 w-3.5" />
                )}
                {tx("复制 JSON", "Copy JSON")}
              </Button>
              <Button
                type="button"
                variant="outline"
                size="sm"
                onClick={() =>
                  void handleOpenApi(
                    openApiModal.env === "PROD" ? "TEST" : "PROD"
                  )
                }
              >
                {openApiModal.env === "PROD"
                  ? tx("查看测试环境", "Switch to Staging")
                  : tx("查看生产环境", "Switch to Production")}
              </Button>
            </div>
            <pre className="rounded-md border border-border bg-muted/40 p-3 font-mono-tech text-[11px] overflow-x-auto max-h-[65vh]">
              {JSON.stringify(openApiModal.doc, null, 2)}
            </pre>
          </div>
        )}
      </Modal>

      <ConfirmDialog
        open={Boolean(confirmDeleteEp)}
        onClose={() => setConfirmDeleteEp(null)}
        onConfirm={() =>
          confirmDeleteEp ? void handleDelete(confirmDeleteEp) : undefined
        }
        loading={Boolean(confirmDeleteEp && busyId === confirmDeleteEp.id)}
        title={tx("确认删除端点", "Confirm Endpoint Deletion")}
        description={tx(
          `确定删除端点「${confirmDeleteEp?.title ?? ""}」吗？外部调用将立即失效。`,
          `Delete endpoint "${confirmDeleteEp?.title ?? ""}"? External callers will immediately receive 404.`
        )}
        confirmLabel={tx("确认删除", "Delete")}
      />
    </div>
  );
}
