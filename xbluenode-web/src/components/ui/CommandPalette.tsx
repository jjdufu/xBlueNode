import { useEffect, useMemo, useRef, useState } from "react";
import { Command } from "cmdk";
import { useNavigate } from "react-router";
import {
  ArrowRight,
  Award,
  BarChart3,
  Blocks,
  Bot,
  Cable,
  Database,
  FileUp,
  GitBranch,
  Languages,
  LayoutDashboard,
  Monitor,
  Plus,
  Search,
  ShieldCheck,
  SunMoon,
  Workflow,
  X,
} from "lucide-react";
import { getAccessToken } from "@/lib/api/auth";
import { analyticsApi, type MetricDef } from "@/lib/api/analytics";
import { listAgents, type AgentRecord } from "@/lib/api/axagent";
import { axhubApi, type Dataset } from "@/lib/api/axhub";
import { axsimApi, type AxSandboxRecord } from "@/lib/api/axsim";
import { xblueApi, type NodeTypeSpec } from "@/lib/api/xblue";
import { xblueviewApi, type DashboardRecord } from "@/lib/api/xblueview";
import { useI18n } from "@/i18n";
import { useTheme } from "@/lib/theme";
import { MODULE_META } from "@/modules/meta";

export interface CommandPaletteProps {
  readonly open: boolean;
  readonly onOpenChange: (open: boolean) => void;
}

interface PaletteEntry {
  readonly id: string;
  readonly zh: string;
  readonly en: string;
  readonly hint?: string;
  readonly keywords: readonly string[];
  readonly icon: typeof Search;
  readonly path?: string;
  readonly action?: "theme" | "locale";
}

interface LiveAssetSnapshot {
  readonly fetchedAt: number;
  readonly nodeTypes: readonly NodeTypeSpec[];
  readonly datasets: readonly Dataset[];
  readonly metrics: readonly MetricDef[];
  readonly dashboards: readonly DashboardRecord[];
  readonly sandboxes: readonly AxSandboxRecord[];
  readonly agents: readonly AgentRecord[];
}

const CACHE_TTL_MS = 30_000;
let cachedAssets: LiveAssetSnapshot | null = null;

function scorePaletteEntry(
  value: string,
  search: string,
  keywords?: string[]
): number {
  const q = search.trim().toLowerCase();
  if (!q) return 1;
  const haystacks = [value, ...(keywords ?? [])].map((s) => s.toLowerCase());
  for (const h of haystacks) {
    if (h === q) return 1;
    if (h.startsWith(q)) return 0.96;
    if (h.includes(q)) return 0.88;
  }
  const combined = haystacks.join(" ");
  let qi = 0;
  for (let i = 0; i < combined.length && qi < q.length; i += 1) {
    if (combined[i] === q[qi]) {
      qi += 1;
    }
  }
  return qi === q.length ? 0.52 : 0;
}

const QUICK_ACTIONS: readonly PaletteEntry[] = [
  {
    id: "act-new-connector",
    zh: "新建连接源",
    en: "New Connector",
    keywords: ["connector", "source", "database", "新建连接源", "数据源", "接入"],
    icon: Cable,
    path: "/hub?tab=connectors",
  },
  {
    id: "act-upload-doc",
    zh: "上传解析文档",
    en: "Ingest Document",
    keywords: ["upload", "document", "pdf", "docx", "上传", "知识文档", "解析"],
    icon: FileUp,
    path: "/hub?tab=knowledge",
  },
  {
    id: "act-new-sql",
    zh: "新建 SQL 查询",
    en: "New SQL Query",
    keywords: ["sql", "query", "chart", "探索", "查询", "制图"],
    icon: BarChart3,
    path: "/analytics?tab=sql",
  },
  {
    id: "act-new-node-type",
    zh: "新建节点模型",
    en: "New Node Type",
    keywords: ["node", "ontology", "schema", "model", "节点", "本体", "建模"],
    icon: Blocks,
    path: "/studio?tab=model",
  },
  {
    id: "act-new-workflow",
    zh: "新建编排流程",
    en: "New Workflow",
    keywords: ["workflow", "flow", "dag", "pipeline", "流程", "编排", "流水线"],
    icon: Workflow,
    path: "/flow?tab=workflows",
  },
  {
    id: "act-new-sandbox",
    zh: "新建推演沙盘",
    en: "New Sandbox",
    keywords: ["sandbox", "simulation", "what-if", "沙盘", "推演", "干预"],
    icon: Plus,
    path: "/sim?tab=simulate",
  },
  {
    id: "act-new-dashboard",
    zh: "新建数字看板",
    en: "New Cockpit",
    keywords: ["dashboard", "cockpit", "twin", "数字看板", "孪生看板", "看板", "驾驶舱"],
    icon: Monitor,
    path: "/view",
  },
  {
    id: "act-theme",
    zh: "切换亮暗主题",
    en: "Toggle Theme",
    keywords: ["theme", "dark", "light", "主题", "暗色", "亮色"],
    icon: SunMoon,
    action: "theme",
  },
  {
    id: "act-locale",
    zh: "切换中英语言",
    en: "Switch Language",
    keywords: ["locale", "language", "english", "chinese", "语言", "中英"],
    icon: Languages,
    action: "locale",
  },
];

const WORKBENCH_VIEWS: readonly PaletteEntry[] = [
  {
    id: "nav-dashboard",
    zh: MODULE_META.dashboard.label,
    en: MODULE_META.dashboard.enLabel,
    keywords: ["overview", "home", "template", "概览", "首页", "向导", "模板"],
    icon: LayoutDashboard,
    path: "/",
  },
  {
    id: "nav-hub",
    zh: MODULE_META.axhub.label,
    en: MODULE_META.axhub.enLabel,
    keywords: ["hub", "dataset", "connector", "数据与知识", "数据集", "连接源"],
    icon: Cable,
    path: "/hub",
  },
  {
    id: "nav-hub-datasets",
    zh: "数据与知识 · 数据集",
    en: "Data Hub · Datasets",
    keywords: ["dataset", "table", "lakehouse", "data & knowledge", "数据集", "湖表"],
    icon: Database,
    path: "/hub?tab=datasets",
  },
  {
    id: "nav-hub-knowledge",
    zh: "数据与知识 · 知识文档",
    en: "Data Hub · Knowledge Docs",
    keywords: ["ingest", "document", "knowledge", "data & knowledge", "知识文档", "文档台账"],
    icon: FileUp,
    path: "/hub?tab=knowledge",
  },
  {
    id: "nav-studio",
    zh: MODULE_META.xbluestudio.label,
    en: MODULE_META.xbluestudio.enLabel,
    keywords: ["studio", "modeling", "ontology", "schema", "建模工作室", "本体工作室", "本体", "模型设计", "建模"],
    icon: Blocks,
    path: "/studio",
  },
  {
    id: "nav-studio-explorer",
    zh: "建模工作室 · 图谱浏览",
    en: "Modeling Studio · Graph Explorer",
    keywords: ["graph", "explorer", "topology", "ontology", "建模工作室", "本体工作室", "本体", "图谱浏览", "拓扑", "邻居"],
    icon: GitBranch,
    path: "/studio?tab=explorer",
  },
  {
    id: "nav-analytics",
    zh: MODULE_META.axanalytics.label,
    en: MODULE_META.axanalytics.enLabel,
    keywords: ["analytics", "sql", "chart", "查询与制图", "探索与制图", "查询", "图表"],
    icon: BarChart3,
    path: "/analytics",
  },
  {
    id: "nav-analytics-ask",
    zh: "查询与制图 · 智能问答",
    en: "Analytics & Charts · Intelligent Q&A",
    keywords: ["ask", "qa", "rag", "nl2sql", "explore & charts", "探索与制图", "智能问答", "问数"],
    icon: Search,
    path: "/analytics?tab=ask",
  },
  {
    id: "nav-analytics-pivot",
    zh: "查询与制图 · 透视分析",
    en: "Analytics & Charts · Pivot Analysis",
    keywords: ["pivot", "cross", "explore & charts", "探索与制图", "透视", "透视分析", "交叉表"],
    icon: BarChart3,
    path: "/analytics?tab=pivot",
  },
  {
    id: "nav-analytics-semantic",
    zh: "查询与制图 · 语义指标",
    en: "Analytics & Charts · Semantic Metrics",
    keywords: ["metric", "semantic", "kpi", "explore & charts", "探索与制图", "语义指标", "认证指标"],
    icon: Award,
    path: "/analytics?tab=semantic",
  },
  {
    id: "nav-sim",
    zh: MODULE_META.axsim.label,
    en: MODULE_META.axsim.enLabel,
    keywords: ["sim", "simulation", "sandbox", "causal", "推演沙盘", "因果"],
    icon: GitBranch,
    path: "/sim",
  },
  {
    id: "nav-flow",
    zh: MODULE_META.axflow.label,
    en: MODULE_META.axflow.enLabel,
    keywords: ["flow", "automation", "dag", "编排与告警", "流水线"],
    icon: Workflow,
    path: "/flow",
  },
  {
    id: "nav-flow-alerts",
    zh: "编排与告警 · 规则告警",
    en: "Automation & Alerts · Rules & Alerts",
    keywords: ["alert", "rule", "silence", "规则告警", "告警", "静默"],
    icon: Workflow,
    path: "/flow?tab=alerts",
  },
  {
    id: "nav-flow-approvals",
    zh: "编排与告警 · 审批待办",
    en: "Automation & Alerts · Approvals",
    keywords: ["approval", "inbox", "审批", "待办"],
    icon: Workflow,
    path: "/flow?tab=approvals",
  },
  {
    id: "nav-view",
    zh: MODULE_META.xblueview.label,
    en: MODULE_META.xblueview.enLabel,
    keywords: ["view", "cockpit", "export", "数字看板", "孪生看板", "驾驶舱", "出品"],
    icon: Monitor,
    path: "/view",
  },
  {
    id: "nav-agents",
    zh: MODULE_META.axagent.label,
    en: MODULE_META.axagent.enLabel,
    keywords: ["agent", "mcp", "api", "im", "llm", "智能体与接口", "智能体中心", "接口", "模型"],
    icon: Bot,
    path: "/agents",
  },
  {
    id: "nav-governance",
    zh: MODULE_META.governance.label,
    en: MODULE_META.governance.enLabel,
    keywords: ["settings", "governance", "security", "rbac", "系统设置", "系统治理", "权限与安全"],
    icon: ShieldCheck,
    path: "/settings",
  },
  {
    id: "nav-governance-masking",
    zh: "系统设置 · 列脱敏",
    en: "Settings · Column Masking",
    keywords: ["settings", "governance", "masking", "pii", "column", "系统治理", "脱敏", "列脱敏", "敏感"],
    icon: ShieldCheck,
    path: "/settings?section=security&tab=column_policies",
  },
  {
    id: "nav-governance-audit",
    zh: "系统设置 · 审计日志",
    en: "Settings · Audit Logs",
    keywords: ["settings", "governance", "audit", "hash", "chain", "系统治理", "审计", "审计日志"],
    icon: ShieldCheck,
    path: "/settings?section=audit&tab=audit",
  },
  {
    id: "nav-governance-ops",
    zh: "系统设置 · 任务与备份",
    en: "Settings · Jobs & Backups",
    keywords: ["settings", "governance", "ops", "backup", "jobs", "retention", "系统治理", "运维与备份", "运维", "备份", "任务队列"],
    icon: ShieldCheck,
    path: "/settings?section=ops",
  },
];

export function CommandPalette({ open, onOpenChange }: CommandPaletteProps) {
  const navigate = useNavigate();
  const { isZh, toggleLocale, tx } = useI18n();
  const { toggleTheme } = useTheme();
  const [query, setQuery] = useState("");
  const [liveAssets, setLiveAssets] = useState<LiveAssetSnapshot | null>(
    () => cachedAssets
  );
  const inputRef = useRef<HTMLInputElement | null>(null);

  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "k") {
        e.preventDefault();
        onOpenChange(!open);
      } else if (open && e.key === "Escape") {
        e.preventDefault();
        onOpenChange(false);
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [open, onOpenChange]);

  useEffect(() => {
    if (!open) return;
    setQuery("");
    requestAnimationFrame(() => inputRef.current?.focus());

    if (!getAccessToken()) return;
    if (cachedAssets && Date.now() - cachedAssets.fetchedAt < CACHE_TTL_MS) {
      setLiveAssets(cachedAssets);
      return;
    }

    let cancelled = false;
    void Promise.all([
      xblueApi
        .getPublishedSchema("main")
        .then((res) => (Array.isArray(res?.node_types) ? res.node_types : []))
        .catch(() => [] as NodeTypeSpec[]),
      axhubApi
        .listDatasets({ limit: 30 })
        .then((res) => (Array.isArray(res?.items) ? res.items : []))
        .catch(() => [] as Dataset[]),
      analyticsApi
        .listMetrics()
        .then((res) => (Array.isArray(res) ? res : []))
        .catch(() => [] as MetricDef[]),
      xblueviewApi
        .listDashboards({ limit: 30 })
        .then((res) => (Array.isArray(res?.items) ? res.items : []))
        .catch(() => [] as DashboardRecord[]),
      axsimApi
        .listSandboxes()
        .then((res) => (Array.isArray(res) ? res : []))
        .catch(() => [] as AxSandboxRecord[]),
      listAgents()
        .then((res) => (Array.isArray(res) ? res : []))
        .catch(() => [] as AgentRecord[]),
    ]).then(
      ([nodeTypes, datasets, metrics, dashboards, sandboxes, agents]) => {
        if (cancelled) return;
        const snapshot: LiveAssetSnapshot = {
          fetchedAt: Date.now(),
          nodeTypes,
          datasets,
          metrics,
          dashboards,
          sandboxes,
          agents,
        };
        cachedAssets = snapshot;
        setLiveAssets(snapshot);
      }
    );

    return () => {
      cancelled = true;
    };
  }, [open]);

  const domainModelEntries = useMemo<readonly PaletteEntry[]>(() => {
    if (!liveAssets) return [];
    const items: PaletteEntry[] = [];
    for (const nt of liveAssets.nodeTypes.slice(0, 20)) {
      items.push({
        id: `model-node-${nt.name}`,
        zh: `${nt.display_name || nt.name}`,
        en: `${nt.display_name || nt.name}`,
        hint: nt.name,
        keywords: [nt.name, nt.display_name || "", "节点", "node", "ontology"],
        icon: Blocks,
        path: `/studio?tab=model&node=${encodeURIComponent(nt.name)}`,
      });
    }
    for (const ds of liveAssets.datasets.slice(0, 20)) {
      items.push({
        id: `asset-ds-${ds.id}`,
        zh: `${ds.display_name || ds.name}`,
        en: `${ds.display_name || ds.name}`,
        hint: ds.name,
        keywords: [ds.name, ds.display_name || "", "数据集", "dataset"],
        icon: Database,
        path: `/hub?tab=datasets&dataset=${encodeURIComponent(ds.id)}`,
      });
    }
    for (const m of liveAssets.metrics.slice(0, 20)) {
      items.push({
        id: `asset-metric-${m.id}`,
        zh: `${m.display_name || m.name}`,
        en: `${m.display_name || m.name}`,
        hint: m.name,
        keywords: [
          m.name,
          m.display_name || "",
          ...(m.synonyms ?? []),
          "指标",
          "metric",
        ],
        icon: Award,
        path: `/analytics?tab=semantic&metric=${encodeURIComponent(m.name)}`,
      });
    }
    return items;
  }, [liveAssets]);

  const runtimeAssetEntries = useMemo<readonly PaletteEntry[]>(() => {
    if (!liveAssets) return [];
    const items: PaletteEntry[] = [];
    for (const db of liveAssets.dashboards.slice(0, 20)) {
      items.push({
        id: `view-db-${db.slug || db.id}`,
        zh: db.name,
        en: db.name,
        hint: db.slug,
        keywords: [db.name, db.slug, "看板", "驾驶舱", "dashboard", "cockpit"],
        icon: Monitor,
        path: `/view/${encodeURIComponent(db.slug)}`,
      });
    }
    for (const sb of liveAssets.sandboxes.slice(0, 20)) {
      items.push({
        id: `sim-sb-${sb.id}`,
        zh: sb.name,
        en: sb.name,
        hint: tx("沙盘", "Sandbox"),
        keywords: [sb.name, "沙盘", "推演", "sandbox", "simulation"],
        icon: GitBranch,
        path: `/sim?sandbox=${encodeURIComponent(sb.id)}`,
      });
    }
    for (const ag of liveAssets.agents.slice(0, 20)) {
      items.push({
        id: `agent-${ag.id}`,
        zh: ag.name,
        en: ag.name,
        hint: `v${ag.version}`,
        keywords: [ag.name, ag.description || "", "智能体", "agent"],
        icon: Bot,
        path: `/agents?tab=workbench&agent=${encodeURIComponent(ag.id)}`,
      });
    }
    return items;
  }, [liveAssets, tx]);

  if (!open) {
    return null;
  }

  const executeEntry = (entry: PaletteEntry) => {
    onOpenChange(false);
    if (entry.action === "theme") {
      toggleTheme();
      return;
    }
    if (entry.action === "locale") {
      toggleLocale();
      return;
    }
    if (entry.path) {
      void navigate(entry.path);
    }
  };

  const renderGroup = (
    groupId: string,
    heading: string,
    entries: readonly PaletteEntry[]
  ) => {
    if (entries.length === 0) return null;
    return (
      <Command.Group
        key={groupId}
        heading={heading}
        className="[&_[cmdk-group-heading]]:px-2.5 [&_[cmdk-group-heading]]:pt-2 [&_[cmdk-group-heading]]:pb-1 [&_[cmdk-group-heading]]:text-[11px] [&_[cmdk-group-heading]]:font-semibold [&_[cmdk-group-heading]]:text-muted-foreground"
      >
        {entries.map((entry) => {
          const Icon = entry.icon;
          const label = isZh ? entry.zh : entry.en;
          return (
            <Command.Item
              key={entry.id}
              value={`${entry.zh} ${entry.en} ${entry.hint ?? ""}`}
              keywords={[...entry.keywords]}
              data-testid={`cmdk-item-${entry.id}`}
              onSelect={() => executeEntry(entry)}
              className="flex items-center justify-between gap-2 rounded-lg px-2.5 py-2 text-[13px] text-foreground transition-colors cursor-pointer select-none data-[selected=true]:bg-xblue-soft data-[selected=true]:text-xblue data-[selected=true]:font-medium hover:bg-muted/70"
            >
              <span className="flex items-center gap-2 min-w-0 truncate">
                <Icon className="h-3.5 w-3.5 shrink-0 text-xblue" />
                <span className="truncate">{label}</span>
                {entry.hint && (
                  <span className="font-mono-tech text-[11px] text-muted-foreground truncate">
                    {entry.hint}
                  </span>
                )}
              </span>
              <ArrowRight className="h-3.5 w-3.5 opacity-60 shrink-0" />
            </Command.Item>
          );
        })}
      </Command.Group>
    );
  };

  return (
    <div
      className="fixed inset-0 z-50 flex items-start justify-center bg-black/45 pt-[13vh] px-4 backdrop-blur-xs"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) {
          onOpenChange(false);
        }
      }}
    >
      <div
        role="dialog"
        aria-modal="true"
        aria-label={tx("命令面板", "Command Palette")}
        data-testid="global-cmdk-dialog"
        className="w-full max-w-xl overflow-hidden rounded-xl border border-border bg-card text-card-foreground shadow-2xl"
      >
        <Command
          label={tx("命令面板", "Command Palette")}
          filter={scorePaletteEntry}
          loop
          className="w-full flex flex-col"
        >
          <div className="flex items-center gap-2.5 border-b border-border px-3.5 py-2.5">
            <Search className="h-4 w-4 text-xblue shrink-0" />
            <Command.Input
              ref={inputRef}
              data-testid="global-cmdk-input"
              value={query}
              onValueChange={setQuery}
              placeholder={tx(
                "搜索视图、模型、数据集、指标与看板…",
                "Search views, models, datasets, metrics, cockpits…"
              )}
              className="flex-1 bg-transparent text-[13px] text-foreground placeholder:text-muted-foreground focus:outline-none"
            />
            <button
              type="button"
              onClick={() => onOpenChange(false)}
              aria-label={tx("关闭", "Close")}
              className="rounded-md p-1 text-muted-foreground hover:bg-muted hover:text-foreground cursor-pointer"
            >
              <X className="h-3.5 w-3.5" />
            </button>
          </div>

          <Command.List
            data-testid="global-cmdk-list"
            className="max-h-96 overflow-y-auto p-1.5 space-y-1"
          >
            <Command.Empty className="py-8 text-center text-xs text-muted-foreground">
              {tx("无匹配项", "No matching items")}
            </Command.Empty>

            {renderGroup(
              "quick-actions",
              tx("快捷动作", "Quick Actions"),
              QUICK_ACTIONS
            )}
            {renderGroup(
              "workbench-views",
              tx("工作台视图", "Workbenches"),
              WORKBENCH_VIEWS
            )}
            {renderGroup(
              "domain-assets",
              tx("领域模型与数据资产", "Models, Datasets & Metrics"),
              domainModelEntries
            )}
            {renderGroup(
              "runtime-assets",
              tx("看板、沙盘与智能体", "Cockpits, Sandboxes & Agents"),
              runtimeAssetEntries
            )}
          </Command.List>
        </Command>
      </div>
    </div>
  );
}
