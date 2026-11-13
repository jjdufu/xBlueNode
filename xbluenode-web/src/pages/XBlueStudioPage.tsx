import { useEffect, useState } from "react";
import { Plus, Save } from "lucide-react";
import {
  xblueApi,
  type BlueEdgeTypeDef,
  type BlueNode,
  type BlueNodeTypeDef,
} from "@/lib/api";
import { Badge, Button, Card, Input } from "@/components/ui/primitives";

export function XBlueStudioPage() {
  const [nodeTypes, setNodeTypes] = useState<Omit<BlueNodeTypeDef, "id">[]>([]);
  const [edgeTypes, setEdgeTypes] = useState<Omit<BlueEdgeTypeDef, "id">[]>([]);
  const [runtimeNodes, setRuntimeNodes] = useState<BlueNode[]>([]);
  const [newName, setNewName] = useState("");
  const [newDisplay, setNewDisplay] = useState("");
  const [srcType, setSrcType] = useState("");
  const [dstType, setDstType] = useState("");
  const [edgeName, setEdgeName] = useState("SUPPLIES");
  const [status, setStatus] = useState<string | null>(null);

  const load = async () => {
    try {
      const [schema, snap] = await Promise.all([
        xblueApi.getSchema(),
        xblueApi.getNodes(),
      ]);
      setNodeTypes(
        schema.node_types.map((nt) => ({
          name: nt.name,
          display_name: nt.display_name,
          properties: nt.properties,
          agg_rules: nt.agg_rules,
          pos_x: nt.pos_x,
          pos_y: nt.pos_y,
        }))
      );
      setEdgeTypes(
        schema.edge_types.map((et) => ({
          name: et.name,
          src_type_name: et.src_type_name,
          dst_type_name: et.dst_type_name,
          impact_weight: et.impact_weight,
        }))
      );
      setRuntimeNodes(snap.nodes);
    } catch {
      // ignore initial offline state
    }
  };

  useEffect(() => {
    void load();
  }, []);

  const addNodeType = () => {
    if (!newName.trim() || !newDisplay.trim()) return;
    setNodeTypes((prev) => [
      ...prev,
      {
        name: newName.trim(),
        display_name: newDisplay.trim(),
        properties: [{ name: "output", data_type: "number" }],
        agg_rules: [{ target_metric: "output", op: "SUM" }],
        pos_x: 120 + prev.length * 140,
        pos_y: 120,
      },
    ]);
    setNewName("");
    setNewDisplay("");
  };

  const addEdgeType = () => {
    if (!srcType || !dstType || !edgeName.trim()) return;
    setEdgeTypes((prev) => [
      ...prev,
      {
        name: edgeName.trim(),
        src_type_name: srcType,
        dst_type_name: dstType,
        impact_weight: 0.7,
      },
    ]);
  };

  const saveSchema = async () => {
    setStatus("正在发布 xBlueNode 节点与关系模型...");
    try {
      await xblueApi.saveSchema({ node_types: nodeTypes, edge_types: edgeTypes });
      setStatus("xBlueNode Schema 已发布至数据库");
      await load();
    } catch (err) {
      setStatus(err instanceof Error ? err.message : String(err));
    }
  };

  return (
    <div className="space-y-6">
      <div className="flex items-center justify-between border-b border-border pb-4">
        <div>
          <h1 className="text-lg font-semibold tracking-tight">xBlueStudio · 节点与关系建模</h1>
          <p className="text-xs text-muted-foreground mt-0.5">
            定义业务节点类型（BlueNodeType）、因果关系（BlueEdgeType）与动态聚合规则
          </p>
        </div>
        <Button onClick={() => void saveSchema()}>
          <Save className="h-3.5 w-3.5" />
          发布建模 Schema
        </Button>
      </div>

      {status && (
        <Card className="p-3 text-xs font-mono-tech">{status}</Card>
      )}

      <div className="grid grid-cols-1 lg:grid-cols-2 gap-4">
        <Card className="p-4 space-y-3">
          <h2 className="text-sm font-medium">1. 定义业务节点类型 (BlueNodeType)</h2>
          <div className="flex gap-2">
            <Input
              placeholder="英文标识 (如 Factory)"
              value={newName}
              onChange={(e) => setNewName(e.target.value)}
            />
            <Input
              placeholder="中文名称 (如 制造工厂)"
              value={newDisplay}
              onChange={(e) => setNewDisplay(e.target.value)}
            />
            <Button variant="outline" onClick={addNodeType}>
              <Plus className="h-3.5 w-3.5" />
              添加
            </Button>
          </div>
          <div className="space-y-2 pt-2">
            {nodeTypes.map((nt) => (
              <div
                key={nt.name}
                className="flex items-center justify-between rounded border border-border p-2.5 text-xs"
              >
                <div>
                  <span className="font-medium">{nt.display_name}</span>
                  <span className="font-mono-tech text-muted-foreground ml-2">
                    {nt.name}
                  </span>
                </div>
                <Badge variant="xblue">SUM(output)</Badge>
              </div>
            ))}
          </div>
        </Card>

        <Card className="p-4 space-y-3">
          <h2 className="text-sm font-medium">2. 定义因果关系 (BlueEdgeType)</h2>
          <div className="grid grid-cols-3 gap-2">
            <Input
              placeholder="源类型 (如 Factory)"
              value={srcType}
              onChange={(e) => setSrcType(e.target.value)}
            />
            <Input
              placeholder="关系 (如 SUPPLIES)"
              value={edgeName}
              onChange={(e) => setEdgeName(e.target.value)}
            />
            <Input
              placeholder="目标类型 (如 Warehouse)"
              value={dstType}
              onChange={(e) => setDstType(e.target.value)}
            />
          </div>
          <Button variant="outline" onClick={addEdgeType}>
            <Plus className="h-3.5 w-3.5" />
            添加关系连线
          </Button>
          <div className="space-y-2 pt-2">
            {edgeTypes.map((et, idx) => (
              <div
                key={`${et.name}-${idx}`}
                className="flex items-center justify-between rounded border border-border p-2.5 text-xs font-mono-tech"
              >
                <span>
                  {et.src_type_name} --[{et.name}]--&gt; {et.dst_type_name}
                </span>
                <Badge variant="neutral">w={et.impact_weight}</Badge>
              </div>
            ))}
          </div>
        </Card>
      </div>

      <Card className="p-4 space-y-2">
        <h2 className="text-sm font-medium">已注册运行时节点实例 ({runtimeNodes.length})</h2>
        <div className="grid grid-cols-1 md:grid-cols-3 gap-2 text-xs">
          {runtimeNodes.map((node) => (
            <div key={node.id} className="rounded border border-border p-2.5">
              <div className="flex items-center justify-between">
                <span className="font-medium">{node.title}</span>
                <Badge variant="outline">{node.type_name}</Badge>
              </div>
              <div className="font-mono-tech text-[11px] text-muted-foreground mt-1">
                {JSON.stringify(node.metrics)}
              </div>
            </div>
          ))}
        </div>
      </Card>
    </div>
  );
}
