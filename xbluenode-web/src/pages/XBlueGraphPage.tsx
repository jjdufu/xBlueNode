import { useEffect, useMemo, useState } from "react";
import { RefreshCw } from "lucide-react";
import { api, type TopologyGraph, type TopologyNode } from "@/lib/api";
import { Badge, Button, Card } from "@/components/ui/primitives";

export function XBlueGraphPage() {
  const [graph, setGraph] = useState<TopologyGraph | null>(null);
  const [selected, setSelected] = useState<TopologyNode | null>(null);
  const [loading, setLoading] = useState(false);

  const load = async () => {
    setLoading(true);
    try {
      const data = await api.topology();
      setGraph(data);
    } catch {
      setGraph({ nodes: [], edges: [] });
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    void load();
  }, []);

  const layout = useMemo(() => {
    if (!graph || graph.nodes.length === 0) return new Map<string, { x: number; y: number }>();
    const map = new Map<string, { x: number; y: number }>();
    const cx = 360;
    const cy = 230;
    const r = 170;
    graph.nodes.forEach((n, idx) => {
      const angle = (2 * Math.PI * idx) / graph.nodes.length;
      map.set(n.id, {
        x: cx + r * Math.cos(angle),
        y: cy + r * Math.sin(angle),
      });
    });
    return map;
  }, [graph]);

  return (
    <div className="space-y-6">
      <div className="flex items-center justify-between border-b border-border pb-4">
        <div>
          <h1 className="text-lg font-semibold tracking-tight">xBlueGraph · 拓扑浏览器</h1>
          <p className="text-xs text-muted-foreground mt-0.5">
            实时查看超图中的文档（DOC）与高频实体（ENTITY）关系网络
          </p>
        </div>
        <Button variant="outline" onClick={() => void load()} disabled={loading}>
          <RefreshCw className={`h-3.5 w-3.5 ${loading ? "animate-spin" : ""}`} />
          刷新拓扑
        </Button>
      </div>

      <div className="grid grid-cols-1 lg:grid-cols-12 gap-4">
        <Card className="lg:col-span-8 p-2 overflow-hidden">
          {!graph || graph.nodes.length === 0 ? (
            <div className="h-[460px] flex items-center justify-center text-xs text-muted-foreground">
              暂无图谱节点，请先在「AxIngest · 知识入库」页面写入文档。
            </div>
          ) : (
            <svg viewBox="0 0 720 460" className="w-full h-[460px] bg-background">
              {graph.edges.map((edge) => {
                const p1 = layout.get(edge.source);
                const p2 = layout.get(edge.target);
                if (!p1 || !p2) return null;
                return (
                  <line
                    key={edge.id}
                    x1={p1.x}
                    y1={p1.y}
                    x2={p2.x}
                    y2={p2.y}
                    stroke="currentColor"
                    strokeOpacity={0.2}
                    strokeWidth={1}
                  />
                );
              })}

              {graph.nodes.map((node) => {
                const pos = layout.get(node.id);
                if (!pos) return null;
                const isSelected = selected?.id === node.id;
                const isEntity = node.node_type === "ENTITY";
                return (
                  <g
                    key={node.id}
                    transform={`translate(${pos.x}, ${pos.y})`}
                    onClick={() => setSelected(node)}
                    className="cursor-pointer"
                  >
                    <circle
                      r={isEntity ? 10 : 12}
                      fill={
                        isSelected
                          ? "var(--xblue)"
                          : isEntity
                            ? "var(--background)"
                            : "var(--foreground)"
                      }
                      stroke={isSelected ? "var(--xblue)" : "var(--foreground)"}
                      strokeWidth={1.5}
                    />
                    <text
                      y={22}
                      textAnchor="middle"
                      className="text-[10px] fill-foreground font-mono-tech"
                    >
                      {node.label.slice(0, 12)}
                    </text>
                  </g>
                );
              })}
            </svg>
          )}
        </Card>

        <Card className="lg:col-span-4 p-4 space-y-3">
          <h2 className="text-sm font-medium border-b border-border pb-2">节点详情</h2>
          {selected ? (
            <div className="space-y-2 text-xs">
              <div className="flex items-center justify-between">
                <span className="text-muted-foreground">类型</span>
                <Badge variant="xblue">{selected.node_type}</Badge>
              </div>
              <div className="flex items-center justify-between">
                <span className="text-muted-foreground">名称</span>
                <span className="font-medium">{selected.label}</span>
              </div>
              <div className="flex items-center justify-between">
                <span className="text-muted-foreground">频次 / 权重</span>
                <span className="font-mono-tech">{selected.weight}</span>
              </div>
              <div className="pt-2 border-t border-border">
                <span className="text-muted-foreground block mb-1">UUID</span>
                <span className="font-mono-tech text-[11px] break-all">
                  {selected.id}
                </span>
              </div>
            </div>
          ) : (
            <p className="text-xs text-muted-foreground">
              点击左侧拓扑画布中的任意节点查看属性与频次信息。
            </p>
          )}
        </Card>
      </div>
    </div>
  );
}
