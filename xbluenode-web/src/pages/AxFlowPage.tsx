import { useEffect, useState } from "react";
import { Play, Plus } from "lucide-react";
import { xblueApi, type AxFlowDef, type AxFlowRunResult } from "@/lib/api";
import { Badge, Button, Card, Input } from "@/components/ui/primitives";

export function AxFlowPage() {
  const [flows, setFlows] = useState<AxFlowDef[]>([]);
  const [name, setName] = useState("");
  const [desc, setDesc] = useState("");
  const [runLog, setRunLog] = useState<AxFlowRunResult | null>(null);

  const load = async () => {
    try {
      setFlows(await xblueApi.listFlows());
    } catch {
      // ignore offline
    }
  };

  useEffect(() => {
    void load();
  }, []);

  const create = async () => {
    if (!name.trim()) return;
    await xblueApi.createFlow({
      name: name.trim(),
      description: desc.trim() || "数据摄取 → AI 清洗 → 节点映射 → 规则触发",
      steps: [
        { id: "s1", kind: "INGEST", label: "多源数据摄取", config: {} },
        { id: "s2", kind: "TRANSFORM", label: "AI 清洗与标准化", config: {} },
        { id: "s3", kind: "NODE_MAP", label: "写入 xBlueNode 节点库", config: {} },
        { id: "s4", kind: "ACTION", label: "发布 API / MCP Skill", config: {} },
      ],
    });
    setName("");
    setDesc("");
    await load();
  };

  const run = async (id: string) => {
    const res = await xblueApi.runFlow(id);
    setRunLog(res);
    await load();
  };

  return (
    <div className="space-y-6">
      <div className="border-b border-border pb-4">
        <h1 className="text-lg font-semibold tracking-tight">AxFlow · 智能编排流水线</h1>
        <p className="text-xs text-muted-foreground mt-0.5">
          一个流水线 = 一个可运行的业务应用，串联多源摄取、节点映射与自动行动
        </p>
      </div>

      <Card className="p-4 flex flex-col md:flex-row gap-2">
        <Input
          placeholder="流水线名称（如：供应链日度产能同步）"
          value={name}
          onChange={(e) => setName(e.target.value)}
        />
        <Input
          placeholder="描述（可选）"
          value={desc}
          onChange={(e) => setDesc(e.target.value)}
        />
        <Button onClick={() => void create()} disabled={!name.trim()}>
          <Plus className="h-3.5 w-3.5" />
          创建并发布
        </Button>
      </Card>

      <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
        {flows.map((flow) => (
          <Card key={flow.id} className="p-4 space-y-3">
            <div className="flex items-center justify-between">
              <div>
                <div className="text-sm font-medium">{flow.name}</div>
                <div className="text-xs text-muted-foreground">{flow.description}</div>
              </div>
              <Badge variant="xblue">{flow.status}</Badge>
            </div>
            <div className="flex flex-wrap gap-1.5">
              {flow.steps.map((s, idx) => (
                <Badge key={s.id} variant="outline">
                  {idx + 1}. {s.label}
                </Badge>
              ))}
            </div>
            <div className="flex items-center justify-between pt-2 border-t border-border">
              <span className="text-[11px] font-mono-tech text-muted-foreground">
                {flow.last_run_at ? `上次运行: ${flow.last_run_at}` : "尚未运行"}
              </span>
              <Button variant="outline" onClick={() => void run(flow.id)}>
                <Play className="h-3.5 w-3.5" />
                立即执行
              </Button>
            </div>
          </Card>
        ))}
      </div>

      {runLog && (
        <Card className="p-4 space-y-2">
          <div className="text-xs font-medium">执行轨迹 ({runLog.status})</div>
          <div className="font-mono-tech text-xs bg-muted/60 rounded p-3 space-y-1">
            {runLog.logs.map((line) => (
              <div key={line}>{line}</div>
            ))}
          </div>
        </Card>
      )}
    </div>
  );
}
