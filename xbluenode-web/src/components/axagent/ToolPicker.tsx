import { useMemo } from "react";
import { Blocks, Bot, Globe, Plug, Wrench } from "lucide-react";
import type {
  AgentRecord,
  EndpointSpec,
  McpClientRecord,
  ToolRef,
} from "@/lib/api/axagent";
import { Badge, Input, StatusBadge } from "@/components/ui/primitives";
import { useI18n } from "@/i18n";
import { cn } from "@/lib/utils";

export interface BuiltinToolDescriptor {
  name: string;
  label: string;
  enLabel?: string;
  scope: string;
  description: string;
  enDescription?: string;
}

export const PLATFORM_BUILTIN_TOOLS: readonly BuiltinToolDescriptor[] = [
  {
    name: "ask_data",
    label: "智能问数",
    enLabel: "Semantic Data Query",
    scope: "DATA_READ",
    description: "NL2SQL 语义问数，受数据集范围约束",
    enDescription: "NL2SQL semantic query scoped to permitted datasets",
  },
  {
    name: "search_knowledge",
    label: "五路知识检索",
    enLabel: "Hybrid Knowledge Search",
    scope: "DATA_READ",
    description: "超图文档与段落融合召回",
    enDescription: "Five-channel fused hypergraph document & passage recall",
  },
  {
    name: "run_sql",
    label: "受控 SQL 查询",
    enLabel: "Guarded SQL Query",
    scope: "DATA_READ",
    description: "经安全守卫校验执行只读查询（上限 200 行）",
    enDescription: "Read-only SQL query enforced by SQL guard (max 200 rows)",
  },
  {
    name: "query_objects",
    label: "领域对象查询",
    enLabel: "Domain Object Query",
    scope: "DATA_READ",
    description: "查询主图业务对象属性、指标与一跳关联",
    enDescription: "Query published node attributes, metrics, and 1-hop links",
  },
  {
    name: "query_metric",
    label: "语义指标查询",
    enLabel: "Semantic Metric Query",
    scope: "DATA_READ",
    description: "按维度与时间粒度聚合语义指标",
    enDescription: "Aggregate semantic metrics by dimensions and time grain",
  },
  {
    name: "get_node",
    label: "读取图节点",
    enLabel: "Get Graph Node",
    scope: "DATA_READ",
    description: "读取业务图谱节点详情",
    enDescription: "Fetch business graph node details by identifier",
  },
  {
    name: "graph_neighbors",
    label: "图邻域探索",
    enLabel: "Graph Neighbors",
    scope: "DATA_READ",
    description: "查询图节点一跳或多跳邻居关系",
    enDescription: "Explore 1-hop or multi-hop graph neighborhood",
  },
  {
    name: "node_metric_series",
    label: "节点时序指标",
    enLabel: "Node Metric Series",
    scope: "DATA_READ",
    description: "读取业务节点时序指标曲线",
    enDescription: "Read time-series metrics for a business node",
  },
  {
    name: "start_flow",
    label: "触发编排流程",
    enLabel: "Start Flow",
    scope: "FLOW_RUN",
    description: "启动测试或生产编排流水线",
    enDescription: "Trigger staging or production pipeline deployment",
  },
  {
    name: "list_alerts",
    label: "活跃告警查询",
    enLabel: "List Active Alerts",
    scope: "DATA_READ",
    description: "检索未恢复或待确认业务告警",
    enDescription: "List open or acknowledged operational alerts",
  },
  {
    name: "run_simulation",
    label: "因果沙盘推演",
    enLabel: "Run Simulation",
    scope: "SIM_RUN",
    description: "在写时复制沙盘执行干预传播推演",
    enDescription: "Execute causal propagation in copy-on-write sandbox",
  },
];

function sanitizeAsToolName(raw: string): string {
  const cleaned = raw
    .toLowerCase()
    .replace(/[^a-z0-9_]+/g, "_")
    .replace(/^[^a-z]+/, "")
    .slice(0, 63);
  return cleaned.length > 0 ? cleaned : "specialist_agent";
}

export interface ToolPickerProps {
  value: ToolRef[];
  onChange: (next: ToolRef[]) => void;
  endpoints: EndpointSpec[];
  mcpClients: McpClientRecord[];
  agents: AgentRecord[];
  currentAgentId?: string | null;
  disabled?: boolean;
}

export function ToolPicker({
  value,
  onChange,
  endpoints,
  mcpClients,
  agents,
  currentAgentId,
  disabled = false,
}: ToolPickerProps) {
  const { isZh, tx, tEnum } = useI18n();

  const builtinCatalog = useMemo(() => {
    const known = new Set(PLATFORM_BUILTIN_TOOLS.map((t) => t.name));
    const extras: BuiltinToolDescriptor[] = [];
    for (const ref of value) {
      if (ref.kind === "BUILTIN" && !known.has(ref.name)) {
        known.add(ref.name);
        extras.push({
          name: ref.name,
          label: ref.name,
          enLabel: ref.name,
          scope: "DATA_READ",
          description: "平台内置工具",
          enDescription: "Built-in platform tool",
        });
      }
    }
    return [...PLATFORM_BUILTIN_TOOLS, ...extras];
  }, [value]);

  const candidateSubAgents = useMemo(
    () => agents.filter((ag) => ag.id !== currentAgentId),
    [agents, currentAgentId]
  );

  const syncedMcpTools = useMemo(() => {
    const list: Array<{
      clientName: string;
      toolName: string;
      description: string;
    }> = [];
    for (const client of mcpClients) {
      const allowedSet =
        client.allowed_tools.length > 0 ? new Set(client.allowed_tools) : null;
      for (const tool of client.tools_cache) {
        if (!allowedSet || allowedSet.has(tool.name)) {
          list.push({
            clientName: client.name,
            toolName: tool.name,
            description: tool.description,
          });
        }
      }
    }
    return list;
  }, [mcpClients]);

  const isBuiltinSelected = (name: string) =>
    value.some((t) => t.kind === "BUILTIN" && t.name === name);

  const toggleBuiltin = (name: string) => {
    if (disabled) return;
    if (isBuiltinSelected(name)) {
      onChange(value.filter((t) => !(t.kind === "BUILTIN" && t.name === name)));
    } else {
      onChange([...value, { kind: "BUILTIN", name }]);
    }
  };

  const isEndpointSelected = (endpointId: string) =>
    value.some((t) => t.kind === "ENDPOINT" && t.endpoint_id === endpointId);

  const toggleEndpoint = (endpointId: string) => {
    if (disabled) return;
    if (isEndpointSelected(endpointId)) {
      onChange(
        value.filter(
          (t) => !(t.kind === "ENDPOINT" && t.endpoint_id === endpointId)
        )
      );
    } else {
      onChange([...value, { kind: "ENDPOINT", endpoint_id: endpointId }]);
    }
  };

  const isMcpSelected = (client: string, tool: string) =>
    value.some(
      (t) => t.kind === "MCP" && t.client === client && t.tool === tool
    );

  const toggleMcp = (client: string, tool: string) => {
    if (disabled) return;
    if (isMcpSelected(client, tool)) {
      onChange(
        value.filter(
          (t) => !(t.kind === "MCP" && t.client === client && t.tool === tool)
        )
      );
    } else {
      onChange([...value, { kind: "MCP", client, tool }]);
    }
  };

  const findAgentRef = (agentId: string) =>
    value.find(
      (t): t is Extract<ToolRef, { kind: "AGENT" }> =>
        t.kind === "AGENT" && t.agent_id === agentId
    );

  const toggleSubAgent = (agent: AgentRecord) => {
    if (disabled) return;
    const existing = findAgentRef(agent.id);
    if (existing) {
      onChange(
        value.filter((t) => !(t.kind === "AGENT" && t.agent_id === agent.id))
      );
    } else {
      onChange([
        ...value,
        {
          kind: "AGENT",
          agent_id: agent.id,
          as_tool_name: sanitizeAsToolName(agent.name),
        },
      ]);
    }
  };

  const updateSubAgentToolName = (agentId: string, rawName: string) => {
    if (disabled) return;
    const sanitized = rawName
      .toLowerCase()
      .replace(/[^a-z0-9_]/g, "")
      .slice(0, 63);
    onChange(
      value.map((t) =>
        t.kind === "AGENT" && t.agent_id === agentId
          ? { ...t, as_tool_name: sanitized || "specialist" }
          : t
      )
    );
  };

  return (
    <div className="space-y-4" data-testid="tool-picker">
      {/* 1. Built-in Tools */}
      <div className="space-y-2">
        <div className="flex items-center gap-1.5 text-xs font-medium text-foreground">
          <Wrench className="h-3.5 w-3.5 text-xblue" />
          {tx("1. 平台内置工具", "1. Built-in Tools")}
        </div>
        <div className="grid grid-cols-1 sm:grid-cols-2 gap-2">
          {builtinCatalog.map((item) => {
            const checked = isBuiltinSelected(item.name);
            return (
              <label
                key={item.name}
                data-testid={`tool-builtin-${item.name}`}
                className={cn(
                  "flex items-start gap-2.5 rounded-md border p-2.5 text-xs transition-colors cursor-pointer",
                  checked
                    ? "border-xblue bg-xblue-soft/40 text-foreground"
                    : "border-border bg-background hover:bg-muted/50 text-foreground"
                )}
              >
                <input
                  type="checkbox"
                  checked={checked}
                  disabled={disabled}
                  onChange={() => toggleBuiltin(item.name)}
                  className="mt-0.5 accent-[#2563eb]"
                />
                <div className="min-w-0 flex-1 space-y-0.5">
                  <div className="flex items-center justify-between gap-1.5">
                    <span className="font-medium truncate">
                      {isZh ? item.label : item.enLabel ?? item.label}{" "}
                      <span className="font-mono-tech text-[11px] text-muted-foreground">
                        {item.name}
                      </span>
                    </span>
                    <Badge variant="outline" title={item.scope}>
                      {tEnum(item.scope)}
                    </Badge>
                  </div>
                  <p className="text-[11px] text-muted-foreground leading-snug">
                    {isZh
                      ? item.description
                      : item.enDescription ?? item.description}
                  </p>
                </div>
              </label>
            );
          })}
        </div>
      </div>

      {/* 2. Published Endpoints */}
      <div className="space-y-2">
        <div className="flex items-center gap-1.5 text-xs font-medium text-foreground">
          <Globe className="h-3.5 w-3.5 text-xblue" />
          {tx("2. 已发布能力端点", "2. Published Endpoints")}
        </div>
        {endpoints.length === 0 ? (
          <div className="rounded-md border border-dashed border-border p-3 text-xs text-muted-foreground">
            {tx("暂无已发布能力端点", "No published endpoints")}
          </div>
        ) : (
          <div className="grid grid-cols-1 sm:grid-cols-2 gap-2">
            {endpoints.map((ep) => {
              const checked = isEndpointSelected(ep.id);
              return (
                <label
                  key={ep.id}
                  data-testid={`tool-endpoint-${ep.slug}-${ep.environment}`}
                  className={cn(
                    "flex items-start gap-2.5 rounded-md border p-2.5 text-xs transition-colors cursor-pointer",
                    checked
                      ? "border-xblue bg-xblue-soft/40"
                      : "border-border bg-background hover:bg-muted/50"
                  )}
                >
                  <input
                    type="checkbox"
                    checked={checked}
                    disabled={disabled}
                    onChange={() => toggleEndpoint(ep.id)}
                    className="mt-0.5 accent-[#2563eb]"
                  />
                  <div className="min-w-0 flex-1 space-y-0.5">
                    <div className="flex items-center justify-between gap-1.5">
                      <span className="font-medium truncate">{ep.title}</span>
                      <div className="flex items-center gap-1 shrink-0">
                        <StatusBadge status={ep.environment} />
                        <Badge variant="outline">{tEnum(ep.kind)}</Badge>
                      </div>
                    </div>
                    <div className="text-[11px] font-mono-tech text-muted-foreground truncate">
                      {ep.slug}
                    </div>
                  </div>
                </label>
              );
            })}
          </div>
        )}
      </div>

      {/* 3. External MCP Tools */}
      <div className="space-y-2">
        <div className="flex items-center gap-1.5 text-xs font-medium text-foreground">
          <Plug className="h-3.5 w-3.5 text-xblue" />
          {tx("3. 外部 MCP 工具", "3. External MCP Tools")}
        </div>
        {syncedMcpTools.length === 0 ? (
          <div className="rounded-md border border-dashed border-border p-3 text-xs text-muted-foreground">
            {tx("暂无已同步的远程 MCP 工具", "No synced remote MCP tools")}
          </div>
        ) : (
          <div className="grid grid-cols-1 sm:grid-cols-2 gap-2">
            {syncedMcpTools.map((item) => {
              const checked = isMcpSelected(item.clientName, item.toolName);
              return (
                <label
                  key={`${item.clientName}:${item.toolName}`}
                  data-testid={`tool-mcp-${item.clientName}-${item.toolName}`}
                  className={cn(
                    "flex items-start gap-2.5 rounded-md border p-2.5 text-xs transition-colors cursor-pointer",
                    checked
                      ? "border-xblue bg-xblue-soft/40"
                      : "border-border bg-background hover:bg-muted/50"
                  )}
                >
                  <input
                    type="checkbox"
                    checked={checked}
                    disabled={disabled}
                    onChange={() => toggleMcp(item.clientName, item.toolName)}
                    className="mt-0.5 accent-[#2563eb]"
                  />
                  <div className="min-w-0 flex-1 space-y-0.5">
                    <div className="flex items-center justify-between gap-1.5">
                      <span className="font-mono-tech font-medium truncate">
                        {item.toolName}
                      </span>
                      <Badge variant="outline">{item.clientName}</Badge>
                    </div>
                    {item.description ? (
                      <p className="text-[11px] text-muted-foreground truncate">
                        {item.description}
                      </p>
                    ) : null}
                  </div>
                </label>
              );
            })}
          </div>
        )}
      </div>

      {/* 4. Specialist Sub-Agents */}
      <div className="space-y-2">
        <div className="flex items-center justify-between gap-2">
          <div className="flex items-center gap-1.5 text-xs font-medium text-foreground">
            <Bot className="h-3.5 w-3.5 text-xblue" />
            {tx("4. 专家子智能体协作", "4. Specialist Sub-Agents")}
          </div>
        </div>
        {candidateSubAgents.length === 0 ? (
          <div className="rounded-md border border-dashed border-border p-3 text-xs text-muted-foreground">
            {tx("暂无可挂载的专家智能体", "No specialist agents available")}
          </div>
        ) : (
          <div className="space-y-2">
            {candidateSubAgents.map((ag) => {
              const ref = findAgentRef(ag.id);
              const checked = Boolean(ref);
              return (
                <div
                  key={ag.id}
                  data-testid={`tool-subagent-${ag.name}`}
                  className={cn(
                    "rounded-md border p-2.5 text-xs space-y-2",
                    checked
                      ? "border-xblue bg-xblue-soft/40"
                      : "border-border bg-background"
                  )}
                >
                  <div className="flex items-center justify-between gap-2">
                    <label className="flex items-center gap-2 cursor-pointer min-w-0">
                      <input
                        type="checkbox"
                        checked={checked}
                        disabled={disabled}
                        onChange={() => toggleSubAgent(ag)}
                        className="accent-[#2563eb]"
                      />
                      <Blocks className="h-3.5 w-3.5 text-muted-foreground shrink-0" />
                      <span className="font-medium truncate">{ag.name}</span>
                      <Badge variant={ag.version > 0 ? "xblue" : "outline"}>
                        v{ag.version}
                      </Badge>
                    </label>
                    <span className="text-[11px] text-muted-foreground truncate max-w-[240px]">
                      {ag.description || tx("专家代理", "Specialist")}
                    </span>
                  </div>
                  {checked && ref ? (
                    <div className="flex items-center gap-2 pl-6">
                      <span className="text-[11px] text-muted-foreground shrink-0">
                        {tx("暴露工具名", "Tool Name")}:
                      </span>
                      <Input
                        value={ref.as_tool_name}
                        disabled={disabled}
                        onChange={(e) =>
                          updateSubAgentToolName(ag.id, e.target.value)
                        }
                        placeholder="data_analyst"
                        className="h-7 text-xs font-mono-tech max-w-[220px]"
                      />
                    </div>
                  ) : null}
                </div>
              );
            })}
          </div>
        )}
      </div>
    </div>
  );
}
