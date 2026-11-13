import { useEffect, useState } from "react";
import { GitBranch, Play } from "lucide-react";
import {
  xblueApi,
  type AxSandboxRecord,
  type AxSimResult,
  type BlueNode,
} from "@/lib/api";
import { Badge, Button, Card, Input } from "@/components/ui/primitives";

export function AxSimPage() {
  const [sandboxes, setSandboxes] = useState<AxSandboxRecord[]>([]);
  const [nodes, setNodes] = useState<BlueNode[]>([]);
  const [sbName, setSbName] = useState("");
  const [selectedSb, setSelectedSb] = useState<string>("");
  const [targetNode, setTargetNode] = useState<string>("");
  const [deltaRatio, setDeltaRatio] = useState<string>("-0.5");
  const [simResult, setSimResult] = useState<AxSimResult | null>(null);

  const load = async () => {
    try {
      const [sbs, snap] = await Promise.all([
        xblueApi.listSandboxes(),
        xblueApi.getNodes(),
      ]);
      setSandboxes(sbs);
      setNodes(snap.nodes);
      if (sbs[0] && !selectedSb) setSelectedSb(sbs[0].id);
      if (snap.nodes[0] && !targetNode) setTargetNode(snap.nodes[0].id);
    } catch {
      // ignore offline
    }
  };

  useEffect(() => {
    void load();
  }, []);

  const createSandbox = async () => {
    if (!sbName.trim()) return;
    const created = await xblueApi.createSandbox({ name: sbName.trim() });
    setSbName("");
    setSelectedSb(created.id);
    await load();
  };

  const runSimulation = async () => {
    if (!selectedSb || !targetNode) return;
    const ratio = Number.parseFloat(deltaRatio);
    if (Number.isNaN(ratio)) return;
    const res = await xblueApi.simulateSandbox(selectedSb, [
      { target_node_id: targetNode, metric_key: "output", delta_ratio: ratio },
    ]);
    setSimResult(res);
    await load();
  };

  return (
    <div className="space-y-6">
      <div className="border-b border-border pb-4">
        <h1 className="text-lg font-semibold tracking-tight">AxSim · 因果推演沙盘</h1>
        <p className="text-xs text-muted-foreground mt-0.5">
          基于写时复制（CoW）克隆生产节点快照，在隔离分支试算 What-If 因果传导，不污染正式数据
        </p>
      </div>

      <div className="grid grid-cols-1 lg:grid-cols-2 gap-4">
        <Card className="p-4 space-y-3">
          <div className="text-sm font-medium flex items-center gap-2">
            <GitBranch className="h-4 w-4 text-xblue" />
            1. 克隆隔离推演沙盘
          </div>
          <div className="flex gap-2">
            <Input
              placeholder="沙盘分支名（如：华东停产3天应急推演）"
              value={sbName}
              onChange={(e) => setSbName(e.target.value)}
            />
            <Button onClick={() => void createSandbox()} disabled={!sbName.trim()}>
              克隆快照
            </Button>
          </div>
          <div className="space-y-1.5 pt-2">
            {sandboxes.map((sb) => (
              <div
                key={sb.id}
                onClick={() => {
                  setSelectedSb(sb.id);
                  if (sb.sim_result) setSimResult(sb.sim_result);
                }}
                className={`flex items-center justify-between rounded border p-2.5 text-xs cursor-pointer ${
                  selectedSb === sb.id ? "border-xblue bg-xblue-soft/40" : "border-border"
                }`}
              >
                <span className="font-medium">{sb.name}</span>
                <span className="font-mono-tech text-muted-foreground">
                  {sb.node_count} 节点 / {sb.link_count} 关系
                </span>
              </div>
            ))}
          </div>
        </Card>

        <Card className="p-4 space-y-3">
          <div className="text-sm font-medium">2. 施加假设干预（What-If）</div>
          <div className="space-y-2 text-xs">
            <label className="block text-muted-foreground">选择干预节点</label>
            <select
              value={targetNode}
              onChange={(e) => setTargetNode(e.target.value)}
              className="h-9 w-full rounded-md border border-input bg-background px-3 text-xs"
            >
              {nodes.map((n) => (
                <option key={n.id} value={n.id}>
                  [{n.type_name}] {n.title}
                </option>
              ))}
            </select>
            <label className="block text-muted-foreground">
              产能/指标变动比例（例如 `-0.5` 表示下降 50%）
            </label>
            <Input
              value={deltaRatio}
              onChange={(e) => setDeltaRatio(e.target.value)}
            />
          </div>
          <Button
            onClick={() => void runSimulation()}
            disabled={!selectedSb || !targetNode}
          >
            <Play className="h-3.5 w-3.5" />
            运行因果传导推演
          </Button>
        </Card>
      </div>

      {simResult && (
        <Card className="p-4 space-y-3">
          <div className="flex items-center justify-between">
            <h2 className="text-sm font-medium">
              推演影响传导明细（受影响节点：{simResult.affected_count}）
            </h2>
            <Badge variant="xblue">ISOLATED SANDBOX</Badge>
          </div>
          <div className="overflow-x-auto">
            <table className="w-full text-xs">
              <thead>
                <tr className="border-b border-border text-left text-muted-foreground">
                  <th className="py-2">节点名称</th>
                  <th className="py-2">类型</th>
                  <th className="py-2">传导跳数</th>
                  <th className="py-2">基线值</th>
                  <th className="py-2">推演后</th>
                  <th className="py-2">变化量</th>
                </tr>
              </thead>
              <tbody>
                {simResult.impacts.map((row) => (
                  <tr key={`${row.node_id}-${row.metric_key}`} className="border-b border-border/60">
                    <td className="py-2 font-medium">{row.title}</td>
                    <td className="py-2 font-mono-tech">{row.type_name}</td>
                    <td className="py-2 font-mono-tech">{row.hop_distance} hop</td>
                    <td className="py-2 font-mono-tech">{row.before_val.toFixed(1)}</td>
                    <td className="py-2 font-mono-tech">{row.after_val.toFixed(1)}</td>
                    <td className="py-2 font-mono-tech text-xblue">
                      {row.delta >= 0 ? `+${row.delta.toFixed(1)}` : row.delta.toFixed(1)}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </Card>
      )}
    </div>
  );
}
