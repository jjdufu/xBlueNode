import { useEffect, useState } from "react";
import { useNavigate } from "react-router";
import { Bot, CheckCircle2, ExternalLink, Plug } from "lucide-react";
import { useI18n } from "@/i18n";
import { analyticsApi, type MetricDef } from "@/lib/api/analytics";
import type { Environment } from "@/lib/api/enums";
import {
  xblueApi,
  type NodeTypeSpec,
  type PackageSubgraphAgentResponse,
} from "@/lib/api/xblue";
import {
  Badge,
  Button,
  Card,
  Input,
  Modal,
  Select,
  Switch,
} from "@/components/ui/primitives";

export interface StudioAgentPackModalProps {
  open: boolean;
  subgraphSlug: string;
  isDevSandbox: boolean;
  nodeTypes: NodeTypeSpec[];
  defaultSelectedNodes?: string[];
  onPackaged: (res: PackageSubgraphAgentResponse) => Promise<void> | void;
  onClose: () => void;
}

export function StudioAgentPackModal({
  open,
  subgraphSlug,
  isDevSandbox,
  nodeTypes,
  defaultSelectedNodes,
  onPackaged,
  onClose,
}: StudioAgentPackModalProps) {
  const navigate = useNavigate();
  const { tx } = useI18n();

  const [agentName, setAgentName] = useState("");
  const [description, setDescription] = useState("");
  const [selectedNodes, setSelectedNodes] = useState<string[]>([]);
  const [availableMetrics, setAvailableMetrics] = useState<MetricDef[]>([]);
  const [selectedMetrics, setSelectedMetrics] = useState<string[]>([]);
  const [environment, setEnvironment] = useState<Environment>(
    isDevSandbox ? "TEST" : "PROD"
  );
  const [exposeMcp, setExposeMcp] = useState<boolean>(true);
  const [submitting, setSubmitting] = useState(false);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);
  const [packResult, setPackResult] =
    useState<PackageSubgraphAgentResponse | null>(null);

  useEffect(() => {
    if (!open) return;
    setErrorMsg(null);
    setPackResult(null);
    setEnvironment(isDevSandbox ? "TEST" : "PROD");
    setExposeMcp(true);
    const seed =
      defaultSelectedNodes && defaultSelectedNodes.length > 0
        ? defaultSelectedNodes
        : nodeTypes.map((nt) => nt.name);
    setSelectedNodes(seed);

    let cancelled = false;
    void analyticsApi
      .listMetrics()
      .then((items) => {
        if (!cancelled) {
          const valid = Array.isArray(items) ? items : [];
          setAvailableMetrics(valid);
        }
      })
      .catch(() => {
        if (!cancelled) {
          setAvailableMetrics([]);
        }
      });
    return () => {
      cancelled = true;
    };
  }, [open, isDevSandbox, nodeTypes, defaultSelectedNodes]);

  const toggleNode = (name: string) => {
    setSelectedNodes((prev) =>
      prev.includes(name) ? prev.filter((item) => item !== name) : [...prev, name]
    );
  };

  const toggleMetric = (name: string) => {
    setSelectedMetrics((prev) =>
      prev.includes(name) ? prev.filter((item) => item !== name) : [...prev, name]
    );
  };

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    const trimmedName = agentName.trim();
    if (!trimmedName) {
      setErrorMsg(tx("智能体名称不能为空", "Agent name is required"));
      return;
    }
    if (selectedNodes.length === 0 && selectedMetrics.length === 0) {
      setErrorMsg(
        tx(
          "至少勾选一项节点类型或认证指标",
          "Select at least one node type or metric"
        )
      );
      return;
    }
    setSubmitting(true);
    setErrorMsg(null);
    try {
      const effectiveEnv: Environment = isDevSandbox ? "TEST" : environment;
      const res = await xblueApi.packageSubgraphAgent(subgraphSlug, {
        agent_name: trimmedName,
        description: description.trim() || undefined,
        node_types: selectedNodes,
        metric_names: selectedMetrics,
        environment: effectiveEnv,
        expose_mcp: exposeMcp,
        publish_agent: true,
      });
      setPackResult(res);
      await onPackaged(res);
    } catch (err) {
      setErrorMsg(
        err instanceof Error
          ? err.message
          : tx("打包智能体失败", "Failed to package domain agent")
      );
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <Modal
      open={open}
      onClose={onClose}
      testId="studio-agent-pack-modal"
      maxWidthClass="max-w-lg"
      title={
        <>
          <Bot className="h-4 w-4 text-xblue shrink-0" />
          <span>{tx("打包领域智能体", "Package Domain Agent")}</span>
          <Badge variant="outline">{subgraphSlug}</Badge>
        </>
      }
      footer={
        <>
          <Button
            type="button"
            variant="outline"
            onClick={onClose}
            disabled={submitting}
          >
            {tx("取消", "Cancel")}
          </Button>
          <Button
            type="submit"
            form="studio-agent-pack-form"
            variant="primary"
            disabled={submitting}
            data-testid="pack-agent-submit-btn"
          >
            <Bot className="h-3.5 w-3.5" />
            {submitting
              ? tx("生成中…", "Creating…")
              : tx("生成智能体", "Create Agent")}
          </Button>
        </>
      }
    >
      <form
        id="studio-agent-pack-form"
        onSubmit={(e) => void handleSubmit(e)}
        className="space-y-3 text-xs"
      >
        {errorMsg && (
          <div
            role="alert"
            className="rounded border border-red-500/40 bg-red-500/10 px-2.5 py-1.5 text-red-700 dark:text-red-300"
          >
            {errorMsg}
          </div>
        )}

        <div className="grid grid-cols-1 sm:grid-cols-2 gap-2.5">
          <div className="space-y-1">
            <label className="block font-medium">
              {tx("智能体名称", "Agent Name")}
            </label>
            <Input
              data-testid="pack-agent-name-input"
              value={agentName}
              onChange={(e) => setAgentName(e.target.value)}
              placeholder="supply_domain_assistant"
              className="h-8 text-xs font-mono-tech"
            />
          </div>

          <div className="space-y-1">
            <label className="block font-medium">
              {tx("环境", "Environment")}
            </label>
            <Select
              data-testid="pack-agent-env-select"
              value={isDevSandbox ? "TEST" : environment}
              disabled={isDevSandbox}
              onChange={(e) => setEnvironment(e.target.value as Environment)}
              className="h-8 w-full font-mono-tech"
            >
              <option value="PROD">PROD</option>
              <option value="TEST">TEST</option>
            </Select>
          </div>
        </div>

        <div className="space-y-1">
          <label className="block font-medium">
            {tx("领域说明", "Description")}
          </label>
          <Input
            data-testid="pack-agent-desc-input"
            value={description}
            onChange={(e) => setDescription(e.target.value)}
            placeholder={tx(
              "供应链子图受控领域助手",
              "Grounded domain assistant"
            )}
            className="h-8 text-xs"
          />
        </div>

        <div className="space-y-1.5">
          <label className="block font-medium">
            {tx("节点类型", "Node Types")}
          </label>
          {nodeTypes.length === 0 ? (
            <div className="rounded border border-dashed border-border px-2.5 py-2 text-muted-foreground">
              {tx("暂无已定义节点类型", "No node types defined")}
            </div>
          ) : (
            <div className="grid grid-cols-2 gap-1.5 max-h-32 overflow-y-auto rounded border border-border bg-muted/20 p-2">
              {nodeTypes.map((nt) => {
                const checked = selectedNodes.includes(nt.name);
                return (
                  <label
                    key={nt.name}
                    className="flex items-center gap-2 rounded px-1.5 py-1 hover:bg-muted/60 cursor-pointer"
                  >
                    <input
                      type="checkbox"
                      checked={checked}
                      onChange={() => toggleNode(nt.name)}
                      data-testid={`pack-agent-node-${nt.name}`}
                      className="h-3.5 w-3.5 accent-[#2563eb]"
                    />
                    <span className="truncate">
                      {nt.display_name || nt.name}{" "}
                      <span className="font-mono-tech text-[11px] text-muted-foreground">
                        ({nt.name})
                      </span>
                    </span>
                  </label>
                );
              })}
            </div>
          )}
        </div>

        <div className="space-y-1.5">
          <label className="block font-medium">
            {tx("认证指标", "Certified Metrics")}
          </label>
          {availableMetrics.length === 0 ? (
            <div className="rounded border border-dashed border-border px-2.5 py-2 text-muted-foreground">
              {tx("暂无认证指标", "No certified metrics")}
            </div>
          ) : (
            <div className="grid grid-cols-2 gap-1.5 max-h-32 overflow-y-auto rounded border border-border bg-muted/20 p-2">
              {availableMetrics.map((m) => {
                const checked = selectedMetrics.includes(m.name);
                return (
                  <label
                    key={m.name}
                    className="flex items-center gap-2 rounded px-1.5 py-1 hover:bg-muted/60 cursor-pointer"
                  >
                    <input
                      type="checkbox"
                      checked={checked}
                      onChange={() => toggleMetric(m.name)}
                      data-testid={`pack-agent-metric-${m.name}`}
                      className="h-3.5 w-3.5 accent-[#2563eb]"
                    />
                    <span className="truncate">
                      {m.display_name || m.name}{" "}
                      <span className="font-mono-tech text-[11px] text-muted-foreground">
                        ({m.name})
                      </span>
                    </span>
                  </label>
                );
              })}
            </div>
          )}
        </div>

        <div className="flex items-center justify-between gap-3 rounded-md border border-border bg-muted/30 px-3 py-2">
          <div className="flex items-center gap-1.5 font-medium">
            <Plug className="h-3.5 w-3.5 text-xblue" />
            <span>{tx("暴露 MCP", "Expose MCP")}</span>
          </div>
          <Switch
            data-testid="pack-agent-mcp-switch"
            checked={exposeMcp}
            onCheckedChange={setExposeMcp}
            aria-label={tx("暴露 MCP", "Expose MCP")}
          />
        </div>

        {packResult && (
          <Card
            data-testid="pack-agent-result"
            className="p-3 space-y-2 border-xblue/45 bg-xblue-soft/30"
          >
            <div className="flex flex-wrap items-center justify-between gap-2">
              <div className="flex items-center gap-1.5 font-semibold text-foreground">
                <CheckCircle2 className="h-3.5 w-3.5 text-xblue" />
                <span className="font-mono-tech">{packResult.agent_name}</span>
                <Badge variant="xblue">v{packResult.agent_version}</Badge>
              </div>
              <Button
                type="button"
                variant="outline"
                size="sm"
                data-testid="pack-agent-open-chat-btn"
                onClick={() => {
                  const target =
                    packResult.workbench_url ||
                    `/agents?tab=workbench&agent=${encodeURIComponent(packResult.agent_id)}`;
                  onClose();
                  void navigate(target);
                }}
              >
                {tx("打开对话", "Open Chat")}
                <ExternalLink className="h-3 w-3" />
              </Button>
            </div>

            {Array.isArray(packResult.endpoints_upserted) &&
              packResult.endpoints_upserted.length > 0 && (
                <div className="space-y-1">
                  <div className="text-[11px] text-muted-foreground">
                    {tx("已挂载端点", "Upserted Endpoints")}
                  </div>
                  <div className="flex flex-wrap gap-1">
                    {packResult.endpoints_upserted.map((ep) => (
                      <Badge key={ep.id || ep.slug} variant="outline">
                        {ep.slug} · {ep.kind}
                        {ep.expose_mcp ? " · MCP" : ""}
                      </Badge>
                    ))}
                  </div>
                </div>
              )}

            {Array.isArray(packResult.mcp_tool_names) &&
              packResult.mcp_tool_names.length > 0 && (
                <div className="space-y-1">
                  <div className="text-[11px] text-muted-foreground">
                    {tx("MCP 工具", "MCP Tools")}
                  </div>
                  <div className="flex flex-wrap gap-1">
                    {packResult.mcp_tool_names.map((toolName) => (
                      <Badge key={toolName} variant="neutral">
                        {toolName}
                      </Badge>
                    ))}
                  </div>
                </div>
              )}
          </Card>
        )}
      </form>
    </Modal>
  );
}
