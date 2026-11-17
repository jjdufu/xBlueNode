import { useMemo } from "react";
import { Blocks, Bot, Globe, Plug, Wrench } from "lucide-react";
import type {
  AgentRecord,
  EndpointSpec,
  McpClientRecord,
  ToolRef,
} from "@/lib/api/axagent";
import { Badge, Input } from "@/components/ui/primitives";
import { cn } from "@/lib/utils";

export interface BuiltinToolDescriptor {
  name: string;
  label: string;
  scope: string;
  description: string;
}

export const PLATFORM_BUILTIN_TOOLS: readonly BuiltinToolDescriptor[] = [
  {
    name: "ask_data",
    label: "智能问数 (ask_data)",
    scope: "DATA_READ",
    description: "NL2SQL 语义问数，受 knowledge_scope.dataset_names 约束",
  },
  {
    name: "search_knowledge",
    label: "五路知识检索 (search_knowledge)",
    scope: "DATA_READ",
    description: "AxQuery 超图文档与段落融合召回",
  },
  {
    name: "run_sql",
    label: "受控 SQL 查询 (run_sql)",
    scope: "DATA_READ",
    description: "经 sql_guard 校验执行只读查询（最多 200 行）",
  },
  {
    name: "query_objects",
    label: "领域对象查询 (query_objects)",
    scope: "DATA_READ",
    description: "查询主图已发布业务对象属性、指标与 1 跳关联",
  },
  {
    name: "query_metric",
    label: "语义指标查询 (query_metric)",
    scope: "DATA_READ",
    description: "按维度与时间粒度聚合语义指标",
  },
  {
    name: "get_node",
    label: "读取图节点 (get_node)",
    scope: "DATA_READ",
    description: "按节点标识读取业务图谱节点详情",
  },
  {
    name: "graph_neighbors",
    label: "图邻域探索 (graph_neighbors)",
    scope: "DATA_READ",
    description: "查询 AxGraph 节点一跳或多跳邻居关系",
  },
  {
    name: "node_metric_series",
    label: "节点时序指标 (node_metric_series)",
    scope: "DATA_READ",
    description: "读取业务节点时序指标曲线",
  },
  {
    name: "start_flow",
    label: "触发编排流程 (start_flow)",
    scope: "FLOW_RUN",
    description: "启动 AxFlow 测试或正式部署流水线",
  },
  {
    name: "list_alerts",
    label: "活跃告警查询 (list_alerts)",
    scope: "DATA_READ",
    description: "检索当前未恢复或待确认的业务告警",
  },
  {
    name: "run_simulation",
    label: "因果沙盘推演 (run_simulation)",
    scope: "SIM_RUN",
    description: "在写时复制沙盘上执行干预传播推演",
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
}

export function ToolPicker({
  value,
  onChange,
  endpoints,
  mcpClients,
  agents,
  currentAgentId,
}: ToolPickerProps) {
  const builtinCatalog = useMemo(() => {
    const known = new Set(PLATFORM_BUILTIN_TOOLS.map((t) => t.name));
    const extras: BuiltinToolDescriptor[] = [];
    for (const ref of value) {
      if (ref.kind === "BUILTIN" && !known.has(ref.name)) {
        known.add(ref.name);
        extras.push({
          name: ref.name,
          label: ref.name,
          scope: "DATA_READ",
          description: "已挂载平台内置工具",
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
    if (isBuiltinSelected(name)) {
      onChange(value.filter((t) => !(t.kind === "BUILTIN" && t.name === name)));
    } else {
      onChange([...value, { kind: "BUILTIN", name }]);
    }
  };

  const isEndpointSelected = (endpointId: string) =>
    value.some((t) => t.kind === "ENDPOINT" && t.endpoint_id === endpointId);

  const toggleEndpoint = (endpointId: string) => {
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
          1. 平台内置工具 (BUILTIN)
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
                  onChange={() => toggleBuiltin(item.name)}
                  className="mt-0.5 accent-[#2563eb]"
                />
                <div className="min-w-0 flex-1 space-y-0.5">
                  <div className="flex items-center justify-between gap-1.5">
                    <span className="font-mono-tech font-medium truncate">
                      {item.name}
                    </span>
                    <Badge variant="outline">{item.scope}</Badge>
                  </div>
                  <p className="text-[11px] text-muted-foreground leading-snug">
                    {item.description}
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
          2. 已发布能力端点 (ENDPOINT)
        </div>
        {endpoints.length === 0 ? (
          <div className="rounded-md border border-dashed border-border p-3 text-xs text-muted-foreground">
            暂无已发布能力端点，可在「发布」页签创建指标、查询或对象端点。
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
                    onChange={() => toggleEndpoint(ep.id)}
                    className="mt-0.5 accent-[#2563eb]"
                  />
                  <div className="min-w-0 flex-1 space-y-0.5">
                    <div className="flex items-center justify-between gap-1.5">
                      <span className="font-medium truncate">{ep.title}</span>
                      <div className="flex items-center gap-1 shrink-0">
                        <Badge
                          variant={
                            ep.environment === "PROD" ? "xblue" : "neutral"
                          }
                        >
                          {ep.environment}
                        </Badge>
                        <Badge variant="outline">{ep.kind}</Badge>
                      </div>
                    </div>
                    <div className="text-[11px] font-mono-tech text-muted-foreground truncate">
                      slug: {ep.slug}
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
          3. 外部 MCP 同步工具 (MCP)
        </div>
        {syncedMcpTools.length === 0 ? (
          <div className="rounded-md border border-dashed border-border p-3 text-xs text-muted-foreground">
            暂无已同步的外部 MCP 工具，可在「MCP」页签登记服务并点击「同步工具」。
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
            4. 专家子智能体协作 (AGENT · 主管+专家模式，最大嵌套深度 ≤ 2)
          </div>
        </div>
        {candidateSubAgents.length === 0 ? (
          <div className="rounded-md border border-dashed border-border p-3 text-xs text-muted-foreground">
            暂无其他可挂载的专家智能体。
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
                      {ag.description || "专家代理"}
                    </span>
                  </div>
                  {checked && ref ? (
                    <div className="flex items-center gap-2 pl-6">
                      <span className="text-[11px] text-muted-foreground shrink-0">
                        暴露工具名 (as_tool_name):
                      </span>
                      <Input
                        value={ref.as_tool_name}
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
