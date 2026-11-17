import { useEffect, useMemo, useState } from "react";
import { z } from "zod";
import {
  Bot,
  History,
  Link2,
  Plus,
  Rocket,
  Save,
  Trash2,
  X,
} from "lucide-react";
import type { Permission } from "@/lib/api/enums";
import {
  createAgent,
  deleteAgent,
  listAgentVersions,
  publishAgent,
  updateAgent,
  type AgentRecord,
  type AgentSpec,
  type AgentVersionRecord,
  type EndpointSpec,
  type LlmProviderRecord,
  type LlmPurpose,
  type McpClientRecord,
  type ToolRef,
} from "@/lib/api/axagent";
import { Badge, Button, Card, Input, Textarea } from "@/components/ui/primitives";
import { cn } from "@/lib/utils";
import { EmbedDialog } from "./EmbedDialog";
import { ToolPicker } from "./ToolPicker";

const AgentFormSchema = z.object({
  name: z
    .string()
    .trim()
    .min(1, "智能体名称不能为空")
    .max(64, "智能体名称不超过 64 字符"),
  description: z.string().max(500),
  system_prompt: z.string().min(1, "系统提示词不能为空"),
  temperature: z.number().min(0).max(2),
  max_steps: z.number().int().min(1).max(20),
  memory_turns: z.number().int().min(1).max(50),
});

const PURPOSES: readonly LlmPurpose[] = [
  "AGENT",
  "CHAT",
  "NL2SQL",
  "SUMMARIZE",
  "EXTRACT",
  "EMBED",
];

function createDefaultSpec(): AgentSpec {
  return {
    system_prompt:
      "你是企业业务智能助手，优先通过已挂载工具核实实时数据与制度依据后再给出结论。",
    provider_purpose: "AGENT",
    provider_id: null,
    temperature: 0.2,
    max_steps: 8,
    memory_turns: 10,
    tools: [
      { kind: "BUILTIN", name: "ask_data" },
      { kind: "BUILTIN", name: "search_knowledge" },
    ],
    approval_required_tools: [],
    knowledge_scope: {
      doc_title_prefix: null,
      dataset_names: [],
    },
    output_schema: null,
  };
}

export interface AgentEditorProps {
  agents: AgentRecord[];
  providers: LlmProviderRecord[];
  endpoints: EndpointSpec[];
  mcpClients: McpClientRecord[];
  userPermissions: Permission[];
  selectedAgentId: string | null;
  onSelectAgentId: (id: string | null) => void;
  onRefresh: () => Promise<void>;
}

export function AgentEditor({
  agents,
  providers,
  endpoints,
  mcpClients,
  userPermissions,
  selectedAgentId,
  onSelectAgentId,
  onRefresh,
}: AgentEditorProps) {
  const [isCreatingNew, setIsCreatingNew] = useState(false);

  const currentAgent = useMemo(() => {
    if (isCreatingNew) return null;
    if (selectedAgentId) {
      const match = agents.find((a) => a.id === selectedAgentId);
      if (match) return match;
    }
    return agents[0] ?? null;
  }, [agents, isCreatingNew, selectedAgentId]);

  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [systemPrompt, setSystemPrompt] = useState("");
  const [purpose, setPurpose] = useState<LlmPurpose>("AGENT");
  const [providerId, setProviderId] = useState<string>("");
  const [temperature, setTemperature] = useState("0.2");
  const [maxSteps, setMaxSteps] = useState("8");
  const [memoryTurns, setMemoryTurns] = useState("10");
  const [tools, setTools] = useState<ToolRef[]>([]);
  const [approvalToolsText, setApprovalToolsText] = useState("");
  const [docTitlePrefix, setDocTitlePrefix] = useState("");
  const [datasetNamesText, setDatasetNamesText] = useState("");
  const [outputSchemaText, setOutputSchemaText] = useState("");

  const [saving, setSaving] = useState(false);
  const [publishing, setPublishing] = useState(false);
  const [feedback, setFeedback] = useState<string | null>(null);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);

  const [embedOpen, setEmbedOpen] = useState(false);
  const [versionsOpen, setVersionsOpen] = useState(false);
  const [versionsLoading, setVersionsLoading] = useState(false);
  const [versions, setVersions] = useState<AgentVersionRecord[]>([]);

  useEffect(() => {
    if (currentAgent) {
      setName(currentAgent.name);
      setDescription(currentAgent.description);
      setSystemPrompt(currentAgent.spec.system_prompt);
      setPurpose(currentAgent.spec.provider_purpose);
      setProviderId(currentAgent.spec.provider_id ?? "");
      setTemperature(String(currentAgent.spec.temperature));
      setMaxSteps(String(currentAgent.spec.max_steps));
      setMemoryTurns(String(currentAgent.spec.memory_turns));
      setTools(currentAgent.spec.tools);
      setApprovalToolsText(
        currentAgent.spec.approval_required_tools.join(", ")
      );
      setDocTitlePrefix(
        currentAgent.spec.knowledge_scope.doc_title_prefix ?? ""
      );
      setDatasetNamesText(
        currentAgent.spec.knowledge_scope.dataset_names.join(", ")
      );
      setOutputSchemaText(
        currentAgent.spec.output_schema
          ? JSON.stringify(currentAgent.spec.output_schema, null, 2)
          : ""
      );
    } else {
      const def = createDefaultSpec();
      setName("");
      setDescription("");
      setSystemPrompt(def.system_prompt);
      setPurpose(def.provider_purpose);
      setProviderId("");
      setTemperature(String(def.temperature));
      setMaxSteps(String(def.max_steps));
      setMemoryTurns(String(def.memory_turns));
      setTools(def.tools);
      setApprovalToolsText("");
      setDocTitlePrefix("");
      setDatasetNamesText("");
      setOutputSchemaText("");
    }
    setFeedback(null);
    setErrorMsg(null);
  }, [currentAgent]);

  const mountedToolNames = useMemo(() => {
    const names: string[] = [];
    for (const ref of tools) {
      if (ref.kind === "BUILTIN") names.push(ref.name);
      else if (ref.kind === "AGENT") names.push(ref.as_tool_name);
      else if (ref.kind === "MCP") names.push(ref.tool);
      else {
        const ep = endpoints.find((e) => e.id === ref.endpoint_id);
        if (ep) names.push(ep.slug.replace(/-/g, "_"));
      }
    }
    return Array.from(new Set(names));
  }, [endpoints, tools]);

  const parsedApprovalTools = useMemo(
    () =>
      Array.from(
        new Set(
          approvalToolsText
            .split(",")
            .map((s) => s.trim())
            .filter((s) => s.length > 0)
        )
      ),
    [approvalToolsText]
  );

  const toggleApprovalTool = (toolName: string) => {
    const next = parsedApprovalTools.includes(toolName)
      ? parsedApprovalTools.filter((t) => t !== toolName)
      : [...parsedApprovalTools, toolName];
    setApprovalToolsText(next.join(", "));
  };

  const buildValidatedSpec = (): {
    name: string;
    description: string;
    spec: AgentSpec;
  } | null => {
    const check = AgentFormSchema.safeParse({
      name,
      description,
      system_prompt: systemPrompt,
      temperature: Number(temperature),
      max_steps: Number(maxSteps),
      memory_turns: Number(memoryTurns),
    });
    if (!check.success) {
      setErrorMsg(check.error.issues[0]?.message ?? "表单字段格式有误");
      return null;
    }

    let parsedOutputSchema: Record<string, unknown> | null = null;
    if (outputSchemaText.trim().length > 0) {
      try {
        const rawJson: unknown = JSON.parse(outputSchemaText.trim());
        if (
          typeof rawJson !== "object" ||
          rawJson === null ||
          Array.isArray(rawJson)
        ) {
          setErrorMsg("输出 Schema 必须是合法的 JSON 对象");
          return null;
        }
        parsedOutputSchema = rawJson as Record<string, unknown>;
      } catch {
        setErrorMsg("输出 Schema JSON 解析失败");
        return null;
      }
    }

    const datasetNames = datasetNamesText
      .split(",")
      .map((s) => s.trim())
      .filter((s) => s.length > 0);

    return {
      name: check.data.name,
      description: check.data.description,
      spec: {
        system_prompt: check.data.system_prompt,
        provider_purpose: purpose,
        provider_id: providerId.trim().length > 0 ? providerId.trim() : null,
        temperature: check.data.temperature,
        max_steps: check.data.max_steps,
        memory_turns: check.data.memory_turns,
        tools,
        approval_required_tools: parsedApprovalTools,
        knowledge_scope: {
          doc_title_prefix:
            docTitlePrefix.trim().length > 0 ? docTitlePrefix.trim() : null,
          dataset_names: datasetNames,
        },
        output_schema: parsedOutputSchema,
      },
    };
  };

  const handleSaveDraft = async () => {
    setErrorMsg(null);
    setFeedback(null);
    const built = buildValidatedSpec();
    if (!built) return;

    setSaving(true);
    try {
      if (currentAgent) {
        await updateAgent(currentAgent.id, {
          name: built.name,
          description: built.description,
          spec: built.spec,
        });
        setFeedback(`草稿已保存 (${built.name})`);
      } else {
        const created = await createAgent({
          name: built.name,
          description: built.description,
          spec: built.spec,
        });
        setIsCreatingNew(false);
        onSelectAgentId(created.id);
        setFeedback(`已新建智能体草稿 (${created.name})`);
      }
      await onRefresh();
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : "保存智能体草稿失败");
    } finally {
      setSaving(false);
    }
  };

  const handlePublishVersion = async () => {
    if (!currentAgent) return;
    setErrorMsg(null);
    setFeedback(null);
    const built = buildValidatedSpec();
    if (!built) return;

    setPublishing(true);
    try {
      await updateAgent(currentAgent.id, {
        name: built.name,
        description: built.description,
        spec: built.spec,
      });
      const ver = await publishAgent(currentAgent.id);
      await onRefresh();
      setFeedback(`已发布不可变版本 v${ver.version}`);
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : "发布智能体版本失败");
    } finally {
      setPublishing(false);
    }
  };

  const handleDelete = async () => {
    if (!currentAgent) return;
    setErrorMsg(null);
    try {
      await deleteAgent(currentAgent.id);
      onSelectAgentId(null);
      await onRefresh();
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : "删除智能体失败");
    }
  };

  const handleOpenVersions = async () => {
    if (!currentAgent) return;
    setVersionsOpen(true);
    setVersionsLoading(true);
    try {
      const items = await listAgentVersions(currentAgent.id);
      if (items.length > 0) {
        setVersions(items);
      } else if (currentAgent.version > 0) {
        setVersions([
          {
            tenant_id: currentAgent.tenant_id,
            agent_id: currentAgent.id,
            version: currentAgent.version,
            spec: currentAgent.spec,
            published_by: currentAgent.created_by,
            published_at: currentAgent.updated_at,
          },
        ]);
      } else {
        setVersions([]);
      }
    } catch {
      setVersions([]);
    } finally {
      setVersionsLoading(false);
    }
  };

  return (
    <div className="grid grid-cols-1 lg:grid-cols-12 gap-4">
      {/* Left Agent List */}
      <Card className="lg:col-span-3 p-3.5 space-y-3">
        <div className="flex items-center justify-between">
          <span className="text-xs font-semibold">智能体列表</span>
          <Button
            type="button"
            variant="outline"
            data-testid="new-agent-btn"
            onClick={() => {
              setIsCreatingNew(true);
              onSelectAgentId(null);
            }}
            className="h-7 px-2.5"
          >
            <Plus className="h-3.5 w-3.5" />
            新建
          </Button>
        </div>

        {agents.length === 0 ? (
          <p className="text-xs text-muted-foreground py-4 text-center">
            暂无智能体，请点击新建
          </p>
        ) : (
          <div className="space-y-1.5">
            {agents.map((ag) => {
              const active = !isCreatingNew && currentAgent?.id === ag.id;
              return (
                <button
                  key={ag.id}
                  type="button"
                  data-testid={`agent-list-item-${ag.name}`}
                  onClick={() => {
                    setIsCreatingNew(false);
                    onSelectAgentId(ag.id);
                  }}
                  className={cn(
                    "w-full text-left rounded-md border p-2.5 text-xs transition-colors cursor-pointer space-y-1",
                    active
                      ? "border-xblue bg-xblue-soft/40 text-foreground"
                      : "border-border bg-background hover:bg-muted/50 text-foreground"
                  )}
                >
                  <div className="flex items-center justify-between gap-2">
                    <span className="font-semibold flex items-center gap-1.5 truncate">
                      <Bot className="h-3.5 w-3.5 text-xblue shrink-0" />
                      {ag.name}
                    </span>
                    <Badge variant={ag.version > 0 ? "xblue" : "outline"}>
                      v{ag.version}
                    </Badge>
                  </div>
                  <p className="text-[11px] text-muted-foreground truncate">
                    {ag.description || "未填写描述"}
                  </p>
                </button>
              );
            })}
          </div>
        )}
      </Card>

      {/* Main Editor Form */}
      <Card className="lg:col-span-9 p-5 space-y-5">
        <div className="flex flex-wrap items-center justify-between gap-3 border-b border-border pb-3">
          <div className="flex items-center gap-2">
            <h2 className="text-sm font-semibold">
              {currentAgent
                ? `编辑智能体 · ${currentAgent.name}`
                : "新建智能体草稿"}
            </h2>
            <Badge variant={currentAgent?.version ? "xblue" : "outline"}>
              {currentAgent ? `v${currentAgent.version}` : "未保存"}
            </Badge>
          </div>

          <div className="flex items-center flex-wrap gap-2">
            {currentAgent ? (
              <>
                <Button
                  type="button"
                  variant="outline"
                  data-testid="open-agent-versions-btn"
                  onClick={() => void handleOpenVersions()}
                >
                  <History className="h-3.5 w-3.5" />
                  版本历史
                </Button>
                <Button
                  type="button"
                  variant="outline"
                  data-testid="open-embed-dialog-btn"
                  onClick={() => setEmbedOpen(true)}
                >
                  <Link2 className="h-3.5 w-3.5" />
                  嵌入分享
                </Button>
              </>
            ) : null}
            <Button
              type="button"
              variant="outline"
              data-testid="save-agent-btn"
              disabled={saving}
              onClick={() => void handleSaveDraft()}
            >
              <Save className="h-3.5 w-3.5" />
              {saving ? "保存中…" : "保存草稿"}
            </Button>
            {currentAgent ? (
              <Button
                type="button"
                data-testid="publish-agent-btn"
                disabled={publishing}
                onClick={() => void handlePublishVersion()}
              >
                <Rocket className="h-3.5 w-3.5" />
                {publishing ? "发布中…" : "发布新版本"}
              </Button>
            ) : null}
            {currentAgent ? (
              <Button
                type="button"
                variant="ghost"
                data-testid="delete-agent-btn"
                onClick={() => void handleDelete()}
              >
                <Trash2 className="h-3.5 w-3.5" />
              </Button>
            ) : null}
          </div>
        </div>

        {errorMsg ? (
          <div className="rounded-md border border-border bg-muted px-3 py-2 text-xs text-foreground">
            {errorMsg}
          </div>
        ) : null}
        {feedback ? (
          <div
            data-testid="agent-editor-feedback"
            className="rounded-md border border-xblue/40 bg-xblue-soft/40 px-3 py-2 text-xs text-foreground"
          >
            {feedback}
          </div>
        ) : null}

        {/* Basic Info */}
        <div className="grid grid-cols-1 sm:grid-cols-2 gap-3 text-xs">
          <div className="space-y-1">
            <label className="text-[11px] font-medium text-muted-foreground">
              唯一标识名称 (name)
            </label>
            <Input
              data-testid="agent-name-input"
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder="ops_supervisor"
              className="font-mono-tech text-xs"
            />
          </div>
          <div className="space-y-1">
            <label className="text-[11px] font-medium text-muted-foreground">
              职能说明 (description)
            </label>
            <Input
              data-testid="agent-desc-input"
              value={description}
              onChange={(e) => setDescription(e.target.value)}
              placeholder="运营主管智能体，协调问数专家与制度专家"
              className="text-xs"
            />
          </div>
        </div>

        {/* System Prompt */}
        <div className="space-y-1 text-xs">
          <label className="text-[11px] font-medium text-muted-foreground">
            系统提示词 (system_prompt)
          </label>
          <Textarea
            data-testid="agent-system-prompt-input"
            value={systemPrompt}
            onChange={(e) => setSystemPrompt(e.target.value)}
            rows={4}
            className="text-xs font-mono-tech"
          />
        </div>

        {/* Model Routing & Loop Parameters */}
        <div className="grid grid-cols-2 sm:grid-cols-5 gap-3 text-xs">
          <div className="space-y-1">
            <label className="text-[11px] text-muted-foreground">
              路由用途 (purpose)
            </label>
            <select
              value={purpose}
              onChange={(e) => setPurpose(e.target.value as LlmPurpose)}
              className="h-9 w-full rounded-md border border-input bg-background px-2.5 text-xs font-mono-tech"
            >
              {PURPOSES.map((p) => (
                <option key={p} value={p}>
                  {p}
                </option>
              ))}
            </select>
          </div>
          <div className="space-y-1">
            <label className="text-[11px] text-muted-foreground">
              指定提供方 (可选)
            </label>
            <select
              value={providerId}
              onChange={(e) => setProviderId(e.target.value)}
              className="h-9 w-full rounded-md border border-input bg-background px-2.5 text-xs"
            >
              <option value="">自动优先级故障切换</option>
              {providers.map((p) => (
                <option key={p.id} value={p.id}>
                  {p.name} ({p.model})
                </option>
              ))}
            </select>
          </div>
          <div className="space-y-1">
            <label className="text-[11px] text-muted-foreground">
              温度 (temperature)
            </label>
            <Input
              type="number"
              step="0.1"
              min={0}
              max={2}
              value={temperature}
              onChange={(e) => setTemperature(e.target.value)}
              className="font-mono-tech text-xs"
            />
          </div>
          <div className="space-y-1">
            <label className="text-[11px] text-muted-foreground">
              最大步数 (max_steps)
            </label>
            <Input
              type="number"
              min={1}
              max={20}
              value={maxSteps}
              onChange={(e) => setMaxSteps(e.target.value)}
              className="font-mono-tech text-xs"
            />
          </div>
          <div className="space-y-1">
            <label className="text-[11px] text-muted-foreground">
              记忆轮数 (memory_turns)
            </label>
            <Input
              type="number"
              min={1}
              max={50}
              value={memoryTurns}
              onChange={(e) => setMemoryTurns(e.target.value)}
              className="font-mono-tech text-xs"
            />
          </div>
        </div>

        {/* Multi-source ToolPicker */}
        <div className="border-t border-border pt-4">
          <ToolPicker
            value={tools}
            onChange={setTools}
            endpoints={endpoints}
            mcpClients={mcpClients}
            agents={agents}
            currentAgentId={currentAgent?.id ?? null}
          />
        </div>

        {/* Approval Required Tools & Knowledge Scope */}
        <div className="grid grid-cols-1 sm:grid-cols-2 gap-4 border-t border-border pt-4 text-xs">
          <div className="space-y-2">
            <label className="text-[11px] font-medium text-muted-foreground block">
              需人工审批工具门禁 (approval_required_tools)
            </label>
            {mountedToolNames.length > 0 ? (
              <div className="flex flex-wrap gap-1.5">
                {mountedToolNames.map((tName) => {
                  const checked = parsedApprovalTools.includes(tName);
                  return (
                    <button
                      key={tName}
                      type="button"
                      onClick={() => toggleApprovalTool(tName)}
                      className={cn(
                        "rounded border px-2 py-0.5 text-[11px] font-mono-tech cursor-pointer",
                        checked
                          ? "border-xblue bg-xblue-soft text-xblue"
                          : "border-border bg-background text-muted-foreground"
                      )}
                    >
                      {checked ? "✓ " : ""}
                      {tName}
                    </button>
                  );
                })}
              </div>
            ) : null}
            <Input
              value={approvalToolsText}
              onChange={(e) => setApprovalToolsText(e.target.value)}
              placeholder="start_flow, run_simulation"
              className="font-mono-tech text-xs"
            />
          </div>

          <div className="space-y-2">
            <label className="text-[11px] font-medium text-muted-foreground block">
              知识与问数范围约束 (knowledge_scope)
            </label>
            <Input
              value={docTitlePrefix}
              onChange={(e) => setDocTitlePrefix(e.target.value)}
              placeholder="文档标题前缀过滤 (doc_title_prefix，留空不限)"
              className="text-xs"
            />
            <Input
              value={datasetNamesText}
              onChange={(e) => setDatasetNamesText(e.target.value)}
              placeholder="限定问数数据集 (逗号分隔，如 demo_orders)"
              className="font-mono-tech text-xs"
            />
          </div>
        </div>

        {/* Optional Output Schema */}
        <div className="space-y-1 border-t border-border pt-4 text-xs">
          <label className="text-[11px] font-medium text-muted-foreground">
            结构化输出 JSON Schema (output_schema，可选)
          </label>
          <Textarea
            value={outputSchemaText}
            onChange={(e) => setOutputSchemaText(e.target.value)}
            rows={3}
            placeholder='{"type": "object", "properties": {"summary": {"type": "string"}}, "required": ["summary"]}'
            className="text-xs font-mono-tech"
          />
        </div>
      </Card>

      <EmbedDialog
        open={embedOpen}
        agent={currentAgent}
        userPermissions={userPermissions}
        onClose={() => setEmbedOpen(false)}
      />

      {versionsOpen && currentAgent ? (
        <div
          data-testid="agent-versions-drawer"
          className="fixed inset-0 z-50 flex justify-end bg-black/50"
        >
          <div className="w-full max-w-md bg-background border-l border-border p-5 overflow-y-auto space-y-4">
            <div className="flex items-center justify-between border-b border-border pb-3">
              <div>
                <h3 className="text-sm font-semibold">
                  版本历史 · {currentAgent.name}
                </h3>
                <p className="text-[11px] text-muted-foreground">
                  不可变快照记录 (ax_agent_versions)
                </p>
              </div>
              <Button
                type="button"
                variant="ghost"
                onClick={() => setVersionsOpen(false)}
                className="h-7 w-7 p-0"
              >
                <X className="h-4 w-4" />
              </Button>
            </div>

            {versionsLoading ? (
              <p className="text-xs text-muted-foreground py-6 text-center">
                正在加载版本快照…
              </p>
            ) : versions.length === 0 ? (
              <p className="text-xs text-muted-foreground py-6 text-center">
                该智能体尚未发布正式版本
              </p>
            ) : (
              <div className="space-y-3">
                {versions.map((ver) => (
                  <Card key={ver.version} className="p-3 space-y-2 text-xs">
                    <div className="flex items-center justify-between">
                      <Badge variant="xblue">v{ver.version}</Badge>
                      <span className="text-[11px] text-muted-foreground font-mono-tech">
                        {new Date(ver.published_at).toLocaleString()}
                      </span>
                    </div>
                    <div className="text-[11px] text-muted-foreground">
                      用途: {ver.spec.provider_purpose} · 工具数:{" "}
                      {ver.spec.tools.length} · 步数上限: {ver.spec.max_steps}
                    </div>
                    <pre className="rounded border border-border bg-muted/40 p-2 text-[11px] font-mono-tech max-h-32 overflow-y-auto whitespace-pre-wrap">
                      {ver.spec.system_prompt}
                    </pre>
                  </Card>
                ))}
              </div>
            )}
          </div>
        </div>
      ) : null}
    </div>
  );
}
