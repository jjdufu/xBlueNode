import { useCallback, useEffect, useState } from "react";
import {
  Database,
  FileText,
  GitBranch,
  Layers,
  RefreshCw,
  Sparkles,
} from "lucide-react";
import { useI18n } from "@/i18n";
import { MODULE_META } from "@/modules/meta";
import { api, type HealthInfo, type SystemStats } from "@/lib/api";
import {
  axopsApi,
  type OnboardingStatusResponse,
} from "@/lib/api/axops";
import { OnboardingStrip } from "@/components/dashboard/OnboardingStrip";
import { TemplateHubModal } from "@/components/dashboard/TemplateHubModal";
import {
  Badge,
  Button,
  Card,
  PageHeader,
  StatCard,
  StatusBadge,
} from "@/components/ui/primitives";

export function DashboardPage() {
  const { tx, fmtNum, fmtTime } = useI18n();
  const [health, setHealth] = useState<HealthInfo | null>(null);
  const [stats, setStats] = useState<SystemStats | null>(null);
  const [onboarding, setOnboarding] =
    useState<OnboardingStatusResponse | null>(null);
  const [templateHubOpen, setTemplateHubOpen] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const [h, s, ob] = await Promise.all([
        api.health(),
        api.stats(),
        axopsApi.getOnboardingStatus().catch(() => null),
      ]);
      setHealth(h);
      setStats(s);
      setOnboarding(ob);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const serviceStatus = health
    ? health.status.toLowerCase() === "ok"
      ? "ONLINE"
      : "OFFLINE"
    : undefined;

  const detectedDims = stats?.embedding_dims?.length
    ? stats.embedding_dims
        .map((d) => tx(`${fmtNum(d, 0)} 维`, `${fmtNum(d, 0)}-dim`))
        .join(", ")
    : health?.embedding_dim
      ? tx(
          `${fmtNum(health.embedding_dim, 0)} 维`,
          `${fmtNum(health.embedding_dim, 0)}-dim`
        )
      : tx("待探测", "Pending");

  const metrics = [
    {
      key: "docs",
      label: tx("文档节点", "Documents"),
      value: fmtNum(stats?.doc_count ?? 0, 0),
      icon: <FileText className="h-3.5 w-3.5" />,
    },
    {
      key: "paras",
      label: tx("段落节点", "Paragraphs"),
      value: fmtNum(stats?.para_count ?? 0, 0),
      icon: <Layers className="h-3.5 w-3.5" />,
    },
    {
      key: "entities",
      label: tx("实体节点", "Entities"),
      value: fmtNum(stats?.entity_count ?? 0, 0),
      icon: <Database className="h-3.5 w-3.5" />,
    },
    {
      key: "edges",
      label: tx("超图关联边", "Hyperedges"),
      value: fmtNum(stats?.edge_count ?? 0, 0),
      icon: <GitBranch className="h-3.5 w-3.5" />,
    },
    {
      key: "embeddings",
      label: tx("向量索引", "Embeddings"),
      value: fmtNum(stats?.embedding_count ?? 0, 0),
      icon: <Sparkles className="h-3.5 w-3.5" />,
    },
  ];

  const recallChannels = [
    {
      key: "vector",
      title: tx("语义向量检索", "Vector Similarity"),
      desc: tx("多维稠密向量 HNSW 近邻匹配", "Multi-dimensional HNSW dense recall"),
    },
    {
      key: "bm25",
      title: tx("全文词法检索", "BM25 Lexical"),
      desc: tx("倒排分词索引与词频权重匹配", "Inverted index term-frequency scoring"),
    },
    {
      key: "entity",
      title: tx("种子实体桥接", "Entity Linking"),
      desc: tx("跨文档同义实体与超边关联对齐", "Cross-document seed entity alignment"),
    },
    {
      key: "context",
      title: tx("相邻上下文扩展", "Context Window"),
      desc: tx("命中段落前后相邻章节补全", "Adjacent paragraph sequence expansion"),
    },
    {
      key: "multihop",
      title: tx("多跳关系推理", "Multi-Hop Traversal"),
      desc: tx("实体关系子图多跳路径遍历", "Multi-hop entity relation path reasoning"),
    },
    {
      key: "rrf",
      title: tx("倒数秩融合重排", "RRF Rank Fusion"),
      desc: tx("多通道统一去重与融合排序 (k = 60)", "Unified reciprocal rank fusion (k = 60)"),
    },
  ];

  return (
    <div className="space-y-5" data-testid="dashboard-page">
      <PageHeader
        title={tx(MODULE_META.dashboard.label, MODULE_META.dashboard.enLabel)}
        badges={
          <>
            {serviceStatus && <StatusBadge status={serviceStatus} />}
            {stats?.timestamp && (
              <Badge variant="outline">{fmtTime(stats.timestamp, true)}</Badge>
            )}
          </>
        }
        actions={
          <Button
            variant="outline"
            onClick={() => void load()}
            disabled={loading}
            data-testid="dashboard-refresh-btn"
          >
            <RefreshCw
              className={`h-3.5 w-3.5 ${loading ? "animate-spin" : ""}`}
            />
            {tx("刷新", "Refresh")}
          </Button>
        }
      />

      <OnboardingStrip
        status={onboarding}
        onOpenTemplateHub={() => setTemplateHubOpen(true)}
      />

      <TemplateHubModal
        open={templateHubOpen}
        onClose={() => setTemplateHubOpen(false)}
      />

      {error && (
        <Card className="p-3.5 border-red-500/30 bg-red-500/5 text-xs">
          <span className="font-semibold text-red-600 dark:text-red-400">
            {tx("服务连接异常：", "Service unreachable: ")}
          </span>
          <span>{error}</span>
        </Card>
      )}

      <div className="grid grid-cols-2 md:grid-cols-5 gap-3">
        {metrics.map((m) => (
          <StatCard
            key={m.key}
            label={m.label}
            value={m.value}
            icon={m.icon}
          />
        ))}
      </div>

      <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
        <Card className="p-4 space-y-3">
          <div className="flex items-center justify-between border-b border-border pb-2.5">
            <h2 className="text-sm font-semibold">
              {tx("核心服务运行态", "Core Service Runtime")}
            </h2>
            <StatusBadge status={serviceStatus ?? "OFFLINE"} />
          </div>

          <div className="space-y-2 text-xs">
            <div className="flex items-center justify-between py-1.5 border-b border-border/70">
              <span className="text-muted-foreground">
                {tx("服务标识", "Service")}
              </span>
              <span className="font-mono-tech font-medium">
                {health?.service || "axpoint-core"}
              </span>
            </div>
            <div className="flex items-center justify-between py-1.5 border-b border-border/70">
              <span className="text-muted-foreground">
                {tx("向量模型", "Embedding Model")}
              </span>
              <span className="font-mono-tech font-medium">
                {health?.embedding_model || "-"}
              </span>
            </div>
            <div className="flex items-center justify-between py-1.5 border-b border-border/70">
              <span className="text-muted-foreground">
                {tx("向量维度", "Vector Dimensions")}
              </span>
              <span className="font-mono-tech font-medium">{detectedDims}</span>
            </div>
            <div className="flex items-center justify-between py-1.5">
              <span className="text-muted-foreground">
                {tx("最近统计时间", "Last Snapshot")}
              </span>
              <span className="font-mono-tech text-muted-foreground">
                {fmtTime(stats?.timestamp)}
              </span>
            </div>
          </div>
        </Card>

        <Card className="p-4 space-y-3">
          <div className="flex items-center justify-between border-b border-border pb-2.5">
            <h2 className="text-sm font-semibold">
              {tx("五路融合召回架构", "5-Way Recall Channels")}
            </h2>
            <Badge variant="xblue">{tx("五路融合重排", "5-Way RRF")}</Badge>
          </div>

          <div className="grid grid-cols-1 sm:grid-cols-2 gap-2 text-xs">
            {recallChannels.map((ch, idx) => (
              <div
                key={ch.key}
                className="rounded border border-border bg-background/60 p-2.5 space-y-0.5"
              >
                <div className="flex items-center gap-1.5 font-medium text-foreground">
                  <span className="font-mono-tech text-[10px] text-xblue">
                    {idx < 5 ? `0${idx + 1}` : "RRF"}
                  </span>
                  <span>{ch.title}</span>
                </div>
                <div className="text-[11px] text-muted-foreground leading-snug">
                  {ch.desc}
                </div>
              </div>
            ))}
          </div>
        </Card>
      </div>
    </div>
  );
}
