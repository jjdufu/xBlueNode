import { useEffect, useMemo, useState } from "react";
import { useNavigate } from "react-router";
import {
  Blocks,
  Bot,
  Code2,
  ExternalLink,
  Monitor,
  Sparkles,
} from "lucide-react";
import { useI18n } from "@/i18n";
import {
  listAgents,
  listEndpoints,
  type AgentRecord,
  type EndpointSpec,
} from "@/lib/api/axagent";
import type {
  NodeConsumptionCounts,
  NodeTypeSpec,
  SubgraphConsumptionResponse,
} from "@/lib/api/xblue";
import {
  xblueviewApi,
  type DashboardRecord,
} from "@/lib/api/xblueview";
import { usePrincipal } from "@/lib/permissions";
import { Badge, Button, Card, Drawer } from "@/components/ui/primitives";
import { cn } from "@/lib/utils";
import { StudioAgentPackModal } from "./StudioAgentPackModal";

export interface StudioAppDrawerProps {
  open: boolean;
  subgraphSlug: string;
  isDevSandbox: boolean;
  nodeTypes: NodeTypeSpec[];
  consumption: SubgraphConsumptionResponse | null;
  filterNodeType: string | null;
  onChangeFilterNodeType: (nodeType: string | null) => void;
  onRefreshConsumption: () => Promise<void>;
  onClose: () => void;
}

export function StudioAppDrawer({
  open,
  subgraphSlug,
  isDevSandbox,
  nodeTypes,
  consumption,
  filterNodeType,
  onChangeFilterNodeType,
  onRefreshConsumption,
  onClose,
}: StudioAppDrawerProps) {
  const navigate = useNavigate();
  const { tx, fmtNum } = useI18n();
  const { can } = usePrincipal();
  const canEditStudio = can("STUDIO_EDIT");

  const [packModalOpen, setPackModalOpen] = useState(false);
  const [selectedCockpitNodes, setSelectedCockpitNodes] = useState<string[]>([]);
  const [agentsById, setAgentsById] = useState<Record<string, AgentRecord>>({});
  const [endpointsBySlug, setEndpointsBySlug] = useState<
    Record<string, EndpointSpec>
  >({});
  const [dashboardsBySlug, setDashboardsBySlug] = useState<
    Record<string, DashboardRecord>
  >({});

  useEffect(() => {
    if (!open) return;
    if (filterNodeType && nodeTypes.some((n) => n.name === filterNodeType)) {
      setSelectedCockpitNodes([filterNodeType]);
    } else {
      setSelectedCockpitNodes(nodeTypes.map((n) => n.name));
    }

    void onRefreshConsumption();

    let cancelled = false;
    void Promise.all([
      listAgents().catch(() => [] as AgentRecord[]),
      listEndpoints().catch(() => [] as EndpointSpec[]),
      xblueviewApi
        .listDashboards({ limit: 100 })
        .catch(() => ({ items: [] as DashboardRecord[], next_cursor: null })),
    ]).then(([agentList, endpointList, dashRes]) => {
      if (cancelled) return;
      const nextAgents: Record<string, AgentRecord> = {};
      for (const a of Array.isArray(agentList) ? agentList : []) {
        nextAgents[a.id] = a;
      }
      setAgentsById(nextAgents);

      const nextEndpoints: Record<string, EndpointSpec> = {};
      for (const ep of Array.isArray(endpointList) ? endpointList : []) {
        nextEndpoints[ep.slug] = ep;
      }
      setEndpointsBySlug(nextEndpoints);

      const nextDashboards: Record<string, DashboardRecord> = {};
      for (const d of Array.isArray(dashRes?.items) ? dashRes.items : []) {
        nextDashboards[d.slug] = d;
      }
      setDashboardsBySlug(nextDashboards);
    });

    return () => {
      cancelled = true;
    };
  }, [open, filterNodeType, nodeTypes, onRefreshConsumption]);

  const byNodeType = useMemo<Record<string, NodeConsumptionCounts>>(() => {
    if (
      consumption &&
      consumption.by_node_type &&
      typeof consumption.by_node_type === "object"
    ) {
      return consumption.by_node_type;
    }
    return {};
  }, [consumption]);

  const allTotals = useMemo(() => {
    const epSet = new Set<string>();
    const agSet = new Set<string>();
    const dbSet = new Set<string>();
    for (const counts of Object.values(byNodeType)) {
      for (const s of counts.endpoint_slugs ?? []) epSet.add(s);
      for (const id of counts.agent_ids ?? []) agSet.add(id);
      for (const s of counts.dashboard_slugs ?? []) dbSet.add(s);
    }
    return {
      endpointCount: epSet.size,
      agentCount: agSet.size,
      dashboardCount: dbSet.size,
    };
  }, [byNodeType]);

  const filteredAssets = useMemo(() => {
    const entries = filterNodeType
      ? byNodeType[filterNodeType]
        ? [byNodeType[filterNodeType]!]
        : []
      : Object.values(byNodeType);
    const epSet = new Set<string>();
    const agSet = new Set<string>();
    const dbSet = new Set<string>();
    for (const item of entries) {
      for (const s of item.endpoint_slugs ?? []) epSet.add(s);
      for (const id of item.agent_ids ?? []) agSet.add(id);
      for (const s of item.dashboard_slugs ?? []) dbSet.add(s);
    }
    return {
      endpointSlugs: Array.from(epSet),
      agentIds: Array.from(agSet),
      dashboardSlugs: Array.from(dbSet),
    };
  }, [byNodeType, filterNodeType]);

  const toggleCockpitNode = (name: string) => {
    setSelectedCockpitNodes((prev) =>
      prev.includes(name) ? prev.filter((item) => item !== name) : [...prev, name]
    );
  };

  const handleBuildCockpit = () => {
    if (selectedCockpitNodes.length === 0) return;
    const url = `/view?wizard=ai&subgraph=${encodeURIComponent(subgraphSlug)}&node_types=${encodeURIComponent(selectedCockpitNodes.join(","))}`;
    onClose();
    void navigate(url);
  };

  const consumedNodeNames = useMemo(
    () =>
      Object.entries(byNodeType)
        .filter(
          ([, c]) =>
            (c.endpoint_count ?? 0) +
              (c.agent_count ?? 0) +
              (c.dashboard_count ?? 0) >
            0
        )
        .map(([name]) => name),
    [byNodeType]
  );

  return (
    <>
      <Drawer
        open={open}
        onClose={onClose}
        testId="studio-app-drawer"
        widthClass="max-w-2xl"
        title={
          <>
            <Blocks className="h-4 w-4 text-xblue shrink-0" />
            <span>{tx("应用包装", "Package App")}</span>
            <Badge variant="outline">{subgraphSlug}</Badge>
            {isDevSandbox && (
              <Badge variant="xblue">{tx("演练沙盒", "Sandbox")}</Badge>
            )}
          </>
        }
      >
        <div className="space-y-4">
          {/* Top Dual-Mode Packaging Cards */}
          <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
            {/* 1. Domain Agent Packaging Card */}
            <Card
              data-testid="studio-pack-agent-card"
              className="p-3.5 flex flex-col justify-between gap-3"
            >
              <div className="space-y-2">
                <div className="flex items-center justify-between gap-2">
                  <span className="text-xs font-semibold flex items-center gap-1.5">
                    <Bot className="h-3.5 w-3.5 text-xblue" />
                    {tx("领域智能体", "Domain Agent")}
                  </span>
                  <Badge variant="xblue">
                    {fmtNum(allTotals.agentCount, 0)} {tx("智能体", "Agents")}
                  </Badge>
                </div>

                <div className="flex flex-wrap gap-1.5 text-[11px]">
                  <Badge variant="outline">
                    {tx("节点类型", "Types")} {fmtNum(nodeTypes.length, 0)}
                  </Badge>
                  <Badge variant="outline">
                    {isDevSandbox ? "TEST" : "PROD / TEST"}
                  </Badge>
                  <Badge variant="neutral">MCP</Badge>
                </div>
              </div>

              <Button
                type="button"
                variant="primary"
                size="sm"
                data-testid="open-pack-agent-modal-btn"
                disabled={!canEditStudio || nodeTypes.length === 0}
                onClick={() => setPackModalOpen(true)}
                className="w-full"
              >
                <Bot className="h-3.5 w-3.5" />
                {tx("打包智能体", "Pack Agent")}
              </Button>
            </Card>

            {/* 2. Twin Cockpit Builder Card */}
            <Card
              data-testid="studio-pack-cockpit-card"
              className="p-3.5 flex flex-col justify-between gap-3"
            >
              <div className="space-y-2">
                <div className="flex items-center justify-between gap-2">
                  <span className="text-xs font-semibold flex items-center gap-1.5">
                    <Monitor className="h-3.5 w-3.5 text-xblue" />
                    {tx("孪生驾驶舱", "Twin Cockpit")}
                  </span>
                  <Badge variant="xblue">
                    {fmtNum(allTotals.dashboardCount, 0)} {tx("看板", "Views")}
                  </Badge>
                </div>

                {nodeTypes.length === 0 ? (
                  <div className="text-[11px] text-muted-foreground">
                    {tx("暂无节点类型", "No node types")}
                  </div>
                ) : (
                  <div className="flex flex-wrap gap-1.5 max-h-24 overflow-y-auto">
                    {nodeTypes.map((nt) => {
                      const checked = selectedCockpitNodes.includes(nt.name);
                      return (
                        <label
                          key={nt.name}
                          className={cn(
                            "inline-flex items-center gap-1.5 rounded border px-2 py-0.5 text-[11px] font-mono-tech cursor-pointer",
                            checked
                              ? "border-xblue bg-xblue-soft text-xblue"
                              : "border-border bg-background text-muted-foreground"
                          )}
                        >
                          <input
                            type="checkbox"
                            checked={checked}
                            onChange={() => toggleCockpitNode(nt.name)}
                            data-testid={`cockpit-node-checkbox-${nt.name}`}
                            className="h-3 w-3 accent-[#2563eb]"
                          />
                          <span>{nt.display_name || nt.name}</span>
                        </label>
                      );
                    })}
                  </div>
                )}
              </div>

              <Button
                type="button"
                variant="outline"
                size="sm"
                data-testid="studio-build-cockpit-btn"
                disabled={selectedCockpitNodes.length === 0}
                onClick={handleBuildCockpit}
                className="w-full"
              >
                <Sparkles className="h-3.5 w-3.5 text-xblue" />
                {tx("生成驾驶舱", "Build Cockpit")}
              </Button>
            </Card>
          </div>

          {/* Bottom Section: Linked Downstream Consumers */}
          <div className="space-y-3" data-testid="studio-consumption-list">
            <div className="flex flex-wrap items-center justify-between gap-2 border-b border-border pb-2">
              <span className="text-xs font-semibold">
                {tx("下游关联资产", "Linked Assets")}
              </span>

              {(consumedNodeNames.length > 0 || filterNodeType) && (
                <div className="flex flex-wrap items-center gap-1">
                  <button
                    type="button"
                    data-testid="clear-consumption-filter-btn"
                    onClick={() => onChangeFilterNodeType(null)}
                    className={cn(
                      "rounded px-2 py-0.5 text-[11px] font-medium cursor-pointer",
                      filterNodeType === null
                        ? "bg-xblue-soft text-xblue"
                        : "bg-muted text-muted-foreground hover:text-foreground"
                    )}
                  >
                    {tx("全部", "All")}
                  </button>
                  {Array.from(
                    new Set(
                      filterNodeType
                        ? [filterNodeType, ...consumedNodeNames]
                        : consumedNodeNames
                    )
                  ).map((nodeName) => (
                    <button
                      key={nodeName}
                      type="button"
                      data-testid={`filter-consumption-node-${nodeName}`}
                      onClick={() =>
                        onChangeFilterNodeType(
                          filterNodeType === nodeName ? null : nodeName
                        )
                      }
                      className={cn(
                        "rounded px-2 py-0.5 text-[11px] font-mono-tech cursor-pointer",
                        filterNodeType === nodeName
                          ? "bg-xblue-soft text-xblue font-semibold"
                          : "bg-muted text-muted-foreground hover:text-foreground"
                      )}
                    >
                      {nodeName}
                    </button>
                  ))}
                </div>
              )}
            </div>

            {/* 1. Linked Endpoints */}
            <Card className="p-3 space-y-2">
              <div className="flex items-center justify-between text-xs font-semibold">
                <span className="inline-flex items-center gap-1.5">
                  <Code2 className="h-3.5 w-3.5 text-xblue" />
                  {tx("领域端点", "Endpoints")}
                </span>
                <Badge variant="outline">
                  {fmtNum(filteredAssets.endpointSlugs.length, 0)}
                </Badge>
              </div>
              {filteredAssets.endpointSlugs.length === 0 ? (
                <div className="text-[11px] text-muted-foreground">
                  {tx("暂无关联端点", "No linked endpoints")}
                </div>
              ) : (
                <div className="divide-y divide-border/60">
                  {filteredAssets.endpointSlugs.map((epSlug) => {
                    const ep = endpointsBySlug[epSlug];
                    return (
                      <div
                        key={epSlug}
                        className="py-1.5 flex items-center justify-between gap-2 text-xs"
                      >
                        <div className="min-w-0">
                          <div className="font-mono-tech font-medium truncate">
                            {epSlug}
                          </div>
                          {ep?.title && (
                            <div className="text-[11px] text-muted-foreground truncate">
                              {ep.title}
                            </div>
                          )}
                        </div>
                        <div className="flex items-center gap-1 shrink-0">
                          {ep?.kind && (
                            <Badge variant="outline">{ep.kind}</Badge>
                          )}
                          {ep?.expose_mcp && (
                            <Badge variant="xblue">MCP</Badge>
                          )}
                        </div>
                      </div>
                    );
                  })}
                </div>
              )}
            </Card>

            {/* 2. Linked Agents */}
            <Card className="p-3 space-y-2">
              <div className="flex items-center justify-between text-xs font-semibold">
                <span className="inline-flex items-center gap-1.5">
                  <Bot className="h-3.5 w-3.5 text-xblue" />
                  {tx("领域智能体", "Domain Agents")}
                </span>
                <Badge variant="outline">
                  {fmtNum(filteredAssets.agentIds.length, 0)}
                </Badge>
              </div>
              {filteredAssets.agentIds.length === 0 ? (
                <div className="text-[11px] text-muted-foreground">
                  {tx("暂无关联智能体", "No linked agents")}
                </div>
              ) : (
                <div className="divide-y divide-border/60">
                  {filteredAssets.agentIds.map((agentId) => {
                    const ag = agentsById[agentId];
                    return (
                      <div
                        key={agentId}
                        className="py-1.5 flex items-center justify-between gap-2 text-xs"
                      >
                        <div className="min-w-0 flex items-center gap-1.5">
                          <span className="font-medium truncate">
                            {ag?.name ?? agentId}
                          </span>
                          {ag && (
                            <Badge variant="outline">v{ag.version}</Badge>
                          )}
                        </div>
                        <Button
                          type="button"
                          variant="outline"
                          size="sm"
                          data-testid={`open-agent-chat-btn-${agentId}`}
                          onClick={() => {
                            onClose();
                            void navigate(
                              `/agents?tab=workbench&agent=${encodeURIComponent(agentId)}`
                            );
                          }}
                        >
                          {tx("打开对话", "Open Chat")}
                          <ExternalLink className="h-3 w-3" />
                        </Button>
                      </div>
                    );
                  })}
                </div>
              )}
            </Card>

            {/* 3. Linked Dashboards */}
            <Card className="p-3 space-y-2">
              <div className="flex items-center justify-between text-xs font-semibold">
                <span className="inline-flex items-center gap-1.5">
                  <Monitor className="h-3.5 w-3.5 text-xblue" />
                  {tx("孪生驾驶舱", "Twin Dashboards")}
                </span>
                <Badge variant="outline">
                  {fmtNum(filteredAssets.dashboardSlugs.length, 0)}
                </Badge>
              </div>
              {filteredAssets.dashboardSlugs.length === 0 ? (
                <div className="text-[11px] text-muted-foreground">
                  {tx("暂无关联驾驶舱", "No linked dashboards")}
                </div>
              ) : (
                <div className="divide-y divide-border/60">
                  {filteredAssets.dashboardSlugs.map((dashSlug) => {
                    const dash = dashboardsBySlug[dashSlug];
                    return (
                      <div
                        key={dashSlug}
                        className="py-1.5 flex items-center justify-between gap-2 text-xs"
                      >
                        <div className="min-w-0">
                          <div className="font-medium truncate">
                            {dash?.name ?? dashSlug}
                          </div>
                          <div className="font-mono-tech text-[11px] text-muted-foreground truncate">
                            /view/{dashSlug}
                          </div>
                        </div>
                        <Button
                          type="button"
                          variant="outline"
                          size="sm"
                          data-testid={`open-dashboard-view-btn-${dashSlug}`}
                          onClick={() => {
                            onClose();
                            void navigate(
                              `/view/${encodeURIComponent(dashSlug)}`
                            );
                          }}
                        >
                          {tx("查看看板", "Open View")}
                          <ExternalLink className="h-3 w-3" />
                        </Button>
                      </div>
                    );
                  })}
                </div>
              )}
            </Card>
          </div>
        </div>
      </Drawer>

      <StudioAgentPackModal
        open={packModalOpen}
        subgraphSlug={subgraphSlug}
        isDevSandbox={isDevSandbox}
        nodeTypes={nodeTypes}
        defaultSelectedNodes={
          filterNodeType ? [filterNodeType] : undefined
        }
        onPackaged={async () => {
          await onRefreshConsumption();
        }}
        onClose={() => setPackModalOpen(false)}
      />
    </>
  );
}
