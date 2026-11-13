import { useEffect, useState } from "react";
import { RefreshCw } from "lucide-react";
import { api, type HealthInfo, type SystemStats } from "@/lib/api";
import { Badge, Button, Card } from "@/components/ui/primitives";

export function DashboardPage() {
  const [health, setHealth] = useState<HealthInfo | null>(null);
  const [stats, setStats] = useState<SystemStats | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);

  const load = async () => {
    setLoading(true);
    setError(null);
    try {
      const [h, s] = await Promise.all([api.health(), api.stats()]);
      setHealth(h);
      setStats(s);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    void load();
  }, []);

  const metrics = [
    { label: "文档节点 (Doc)", value: stats?.doc_count ?? 0 },
    { label: "段落节点 (Para)", value: stats?.para_count ?? 0 },
    { label: "实体节点 (Entity)", value: stats?.entity_count ?? 0 },
    { label: "超图关联边 (Edge)", value: stats?.edge_count ?? 0 },
    { label: "向量索引 (Embedding)", value: stats?.embedding_count ?? 0 },
  ];

  return (
    <div className="space-y-6">
      <div className="flex items-center justify-between border-b border-border pb-4">
        <div>
          <h1 className="text-lg font-semibold tracking-tight">系统概览</h1>
          <p className="text-xs text-muted-foreground mt-0.5">
            xBlueNode 动态超图底座与 AxPoint 多路检索引擎实时运行态势
          </p>
        </div>
        <Button variant="outline" onClick={() => void load()} disabled={loading}>
          <RefreshCw className={`h-3.5 w-3.5 ${loading ? "animate-spin" : ""}`} />
          刷新状态
        </Button>
      </div>

      {error && (
        <Card className="p-3.5 border-foreground/30 bg-muted/40 text-xs">
          <span className="font-mono-tech font-semibold">连接提示：</span>
          {error}（请确认 PostgreSQL 与 axpoint-core 已启动）
        </Card>
      )}

      <div className="grid grid-cols-2 md:grid-cols-5 gap-3">
        {metrics.map((m) => (
          <Card key={m.label} className="p-4">
            <div className="text-[11px] text-muted-foreground">{m.label}</div>
            <div className="text-2xl font-semibold font-mono-tech mt-2 tracking-tight">
              {m.value.toLocaleString()}
            </div>
          </Card>
        ))}
      </div>

      <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
        <Card className="p-5 space-y-3">
          <div className="flex items-center justify-between">
            <h2 className="text-sm font-medium">AxPoint Core 服务状态</h2>
            <Badge variant={health?.status === "ok" ? "xblue" : "outline"}>
              {health?.status === "ok" ? "ONLINE" : "OFFLINE"}
            </Badge>
          </div>
          <div className="space-y-2 text-xs">
            <div className="flex justify-between py-1.5 border-b border-border">
              <span className="text-muted-foreground">核心引擎</span>
              <span className="font-mono-tech">Rust 1.100.0 / Axum</span>
            </div>
            <div className="flex justify-between py-1.5 border-b border-border">
              <span className="text-muted-foreground">Embedding 模型</span>
              <span className="font-mono-tech">{health?.embedding_model ?? "-"}</span>
            </div>
            <div className="flex justify-between py-1.5">
              <span className="text-muted-foreground">已探测向量维度</span>
              <span className="font-mono-tech">
                {stats?.embedding_dims.length
                  ? stats.embedding_dims.map((d) => `${d}d`).join(", ")
                  : health?.embedding_dim
                    ? `${health.embedding_dim}d`
                    : "自适应 (待首次写入探测)"}
              </span>
            </div>
          </div>
        </Card>

        <Card className="p-5 space-y-3">
          <h2 className="text-sm font-medium">五路并行召回架构</h2>
          <div className="grid grid-cols-2 gap-2 text-xs">
            {[
              ["1. Vector", "pgvector 多维条件 HNSW"],
              ["2. BM25", "PostgreSQL tsvector + 词频"],
              ["3. Entity Link", "种子实体跨文档桥接"],
              ["4. Context", "NEXT_PARA 相邻段落扩展"],
              ["5. Multi-Hop", "RELATES 实体多跳路径推理"],
              ["RRF Fusion", "k=60 倒数秩融合排序"],
            ].map(([name, desc]) => (
              <div key={name} className="rounded border border-border p-2.5">
                <div className="font-mono-tech font-medium text-[11px]">{name}</div>
                <div className="text-muted-foreground text-[11px] mt-0.5">{desc}</div>
              </div>
            ))}
          </div>
        </Card>
      </div>
    </div>
  );
}
