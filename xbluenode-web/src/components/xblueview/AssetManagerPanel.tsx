import { useCallback, useEffect, useState } from "react";
import { Box, Layers, Plus, Save, Trash2, Upload } from "lucide-react";
import type { AssetKind } from "@/lib/api/enums";
import { xblueApi, type BlueNode } from "@/lib/api/xblue";
import {
  xblueviewApi,
  type AssetRecord,
  type FloorPlacementRecord,
  type Scene3dBindingRecord,
} from "@/lib/api/xblueview";
import { Badge, Button, Card, Input } from "@/components/ui/primitives";

const ASSET_KINDS: readonly AssetKind[] = [
  "FLOOR_PLAN_IMAGE",
  "MODEL_3D",
  "GEOJSON",
  "ICON",
];

export function AssetManagerPanel() {
  const [assets, setAssets] = useState<AssetRecord[]>([]);
  const [nodes, setNodes] = useState<BlueNode[]>([]);
  const [selectedAsset, setSelectedAsset] = useState<AssetRecord | null>(null);
  const [loading, setLoading] = useState(false);

  // Upload form state
  const [uploadKind, setUploadKind] = useState<AssetKind>("FLOOR_PLAN_IMAGE");
  const [uploadName, setUploadName] = useState("");
  const [uploadFile, setUploadFile] = useState<File | null>(null);
  const [uploading, setUploading] = useState(false);

  // 2D placements editor
  const [placements, setPlacements] = useState<
    Omit<FloorPlacementRecord, "asset_id">[]
  >([]);
  const [floorImgUrl, setFloorImgUrl] = useState<string | null>(null);
  const [activeNodeId, setActiveNodeId] = useState<string>("");

  // 3D bindings editor
  const [bindings3d, setBindings3d] = useState<
    Omit<Scene3dBindingRecord, "asset_id">[]
  >([]);
  const [savingBindings, setSavingBindings] = useState(false);
  const [statusMsg, setStatusMsg] = useState<string | null>(null);

  const loadAssetsAndNodes = useCallback(async () => {
    setLoading(true);
    try {
      const [assetRes, nodeRes] = await Promise.all([
        xblueviewApi.listAssets({ limit: 100 }),
        xblueApi
          .listNodes({ limit: 200 })
          .catch(() => ({ items: [], nodes: [], links: [], next_cursor: null })),
      ]);
      setAssets(Array.isArray(assetRes?.items) ? assetRes.items : []);
      const nodeList =
        Array.isArray(nodeRes?.items) && nodeRes.items.length > 0
          ? nodeRes.items
          : Array.isArray(nodeRes?.nodes)
            ? nodeRes.nodes
            : [];
      setNodes(nodeList);
      if (nodeList[0]) {
        setActiveNodeId((prev) => prev || nodeList[0].id);
      }
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void loadAssetsAndNodes();
  }, [loadAssetsAndNodes]);

  useEffect(() => {
    if (!selectedAsset) {
      setPlacements([]);
      setBindings3d([]);
      setFloorImgUrl(null);
      return;
    }

    let cancelled = false;
    let revokedUrl: string | null = null;

    if (selectedAsset.asset_kind === "FLOOR_PLAN_IMAGE") {
      xblueviewApi
        .getFloorPlacements(selectedAsset.id)
        .then((rows) => {
          if (!cancelled) {
            setPlacements(
              rows.map((r) => ({
                node_id: r.node_id,
                x_ratio: r.x_ratio,
                y_ratio: r.y_ratio,
              }))
            );
          }
        })
        .catch(() => {
          if (!cancelled) setPlacements([]);
        });

      xblueviewApi
        .fetchAssetBlobUrl(selectedAsset.id)
        .then((url) => {
          if (cancelled) {
            URL.revokeObjectURL(url);
            return;
          }
          revokedUrl = url;
          setFloorImgUrl(url);
        })
        .catch(() => {
          if (!cancelled) setFloorImgUrl(null);
        });
    } else if (selectedAsset.asset_kind === "MODEL_3D") {
      xblueviewApi
        .getScene3dBindings(selectedAsset.id)
        .then((rows) => {
          if (!cancelled) {
            setBindings3d(
              rows.map((r) => ({
                node_id: r.node_id,
                mesh_name: r.mesh_name,
                pos_x: r.pos_x,
                pos_y: r.pos_y,
                pos_z: r.pos_z,
                scale: r.scale,
              }))
            );
          }
        })
        .catch(() => {
          if (!cancelled) setBindings3d([]);
        });
    }

    return () => {
      cancelled = true;
      if (revokedUrl) URL.revokeObjectURL(revokedUrl);
    };
  }, [selectedAsset]);

  const handleUpload = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!uploadFile) {
      setStatusMsg("请选择要上传的资产文件");
      return;
    }
    const name = uploadName.trim() || uploadFile.name;
    setUploading(true);
    setStatusMsg(null);
    try {
      const created = await xblueviewApi.uploadAsset(
        uploadFile,
        uploadKind,
        name
      );
      setUploadName("");
      setUploadFile(null);
      setStatusMsg(`资产「${created.name}」上传成功`);
      await loadAssetsAndNodes();
      setSelectedAsset(created);
    } catch (err) {
      setStatusMsg(err instanceof Error ? err.message : String(err));
    } finally {
      setUploading(false);
    }
  };

  const handleDeleteAsset = async (assetId: string) => {
    try {
      await xblueviewApi.deleteAsset(assetId);
      if (selectedAsset?.id === assetId) setSelectedAsset(null);
      await loadAssetsAndNodes();
    } catch (err) {
      setStatusMsg(err instanceof Error ? err.message : String(err));
    }
  };

  const handleFloorCanvasClick = (e: React.MouseEvent<HTMLDivElement>) => {
    if (!activeNodeId) return;
    const rect = e.currentTarget.getBoundingClientRect();
    if (rect.width === 0 || rect.height === 0) return;
    const xRatio = Number(
      Math.max(0.02, Math.min(0.98, (e.clientX - rect.left) / rect.width)).toFixed(4)
    );
    const yRatio = Number(
      Math.max(0.02, Math.min(0.98, (e.clientY - rect.top) / rect.height)).toFixed(4)
    );
    setPlacements((prev) => {
      const filtered = prev.filter((p) => p.node_id !== activeNodeId);
      return [...filtered, { node_id: activeNodeId, x_ratio: xRatio, y_ratio: yRatio }];
    });
  };

  const handleSavePlacements = async () => {
    if (!selectedAsset) return;
    setSavingBindings(true);
    setStatusMsg(null);
    try {
      await xblueviewApi.putFloorPlacements(selectedAsset.id, placements);
      setStatusMsg("平面图节点坐标已保存");
    } catch (err) {
      setStatusMsg(err instanceof Error ? err.message : String(err));
    } finally {
      setSavingBindings(false);
    }
  };

  const handleSaveBindings3d = async () => {
    if (!selectedAsset) return;
    setSavingBindings(true);
    setStatusMsg(null);
    try {
      await xblueviewApi.putScene3dBindings(selectedAsset.id, bindings3d);
      setStatusMsg("3D 空间网格绑定已保存");
    } catch (err) {
      setStatusMsg(err instanceof Error ? err.message : String(err));
    } finally {
      setSavingBindings(false);
    }
  };

  const addBinding3dRow = () => {
    if (!activeNodeId) return;
    if (bindings3d.some((b) => b.node_id === activeNodeId)) return;
    setBindings3d((prev) => [
      ...prev,
      {
        node_id: activeNodeId,
        mesh_name: "",
        pos_x: prev.length * 2,
        pos_y: 0.5,
        pos_z: 0,
        scale: 1,
      },
    ]);
  };

  const nodeTitleMap = new Map(nodes.map((n) => [n.id, `${n.title} (${n.node_key})`]));

  return (
    <div className="grid grid-cols-1 lg:grid-cols-12 gap-4" data-testid="asset-manager-panel">
      {/* Left: Upload + Asset List */}
      <div className="lg:col-span-5 space-y-4">
        <Card className="p-4 space-y-3">
          <h3 className="text-xs font-semibold flex items-center gap-1.5">
            <Upload className="h-3.5 w-3.5 text-xblue" />
            上传空间与模型资产
          </h3>
          <form onSubmit={(e) => void handleUpload(e)} className="space-y-2.5 text-xs">
            <div className="grid grid-cols-2 gap-2">
              <div className="space-y-1">
                <label className="text-muted-foreground block">资产类型</label>
                <select
                  value={uploadKind}
                  onChange={(e) => setUploadKind(e.target.value as AssetKind)}
                  className="h-8 w-full rounded-md border border-input bg-background px-2 text-xs font-mono-tech"
                  data-testid="asset-upload-kind"
                >
                  {ASSET_KINDS.map((k) => (
                    <option key={k} value={k}>
                      {k}
                    </option>
                  ))}
                </select>
              </div>
              <div className="space-y-1">
                <label className="text-muted-foreground block">资产名称</label>
                <Input
                  value={uploadName}
                  onChange={(e) => setUploadName(e.target.value)}
                  placeholder="例如 warehouse_twin"
                  className="h-8 text-xs"
                  data-testid="asset-upload-name"
                />
              </div>
            </div>

            <div className="space-y-1">
              <label className="text-muted-foreground block">
                文件（平面图/GeoJSON/Icon ≤ 20MB，glTF .glb ≤ 50MB）
              </label>
              <input
                type="file"
                onChange={(e) => setUploadFile(e.target.files?.[0] ?? null)}
                className="block w-full text-xs file:mr-2 file:rounded file:border file:border-border file:bg-muted file:px-2.5 file:py-1 file:text-xs"
                data-testid="asset-upload-file"
              />
            </div>

            <Button type="submit" disabled={uploading} className="w-full">
              <Upload className="h-3.5 w-3.5" />
              {uploading ? "上传校验中..." : "上传资产"}
            </Button>
          </form>
        </Card>

        <Card className="p-4 space-y-2.5">
          <div className="flex items-center justify-between text-xs">
            <span className="font-semibold">已注册资产 ({assets.length})</span>
            {loading && <span className="text-muted-foreground">加载中...</span>}
          </div>

          {assets.length === 0 ? (
            <div className="py-6 text-center text-xs text-muted-foreground">
              暂无资产，请先上传平面图、GeoJSON 或 .glb 三维模型。
            </div>
          ) : (
            <div className="space-y-1.5 max-h-80 overflow-y-auto">
              {assets.map((asset) => {
                const active = selectedAsset?.id === asset.id;
                return (
                  <div
                    key={asset.id}
                    onClick={() => setSelectedAsset(asset)}
                    className={`rounded border p-2 flex items-center justify-between gap-2 text-xs cursor-pointer ${
                      active
                        ? "border-xblue bg-xblue-soft/50"
                        : "border-border hover:bg-muted/40"
                    }`}
                  >
                    <div className="min-w-0">
                      <div className="font-medium truncate flex items-center gap-1.5">
                        {asset.asset_kind === "MODEL_3D" ? (
                          <Box className="h-3.5 w-3.5 text-xblue shrink-0" />
                        ) : (
                          <Layers className="h-3.5 w-3.5 text-xblue shrink-0" />
                        )}
                        <span className="truncate">{asset.name}</span>
                      </div>
                      <div className="flex items-center gap-1.5 mt-0.5">
                        <Badge variant="outline">{asset.asset_kind}</Badge>
                        <span className="font-mono-tech text-[10px] text-muted-foreground">
                          {asset.id.slice(0, 8)}
                        </span>
                      </div>
                    </div>
                    <button
                      type="button"
                      onClick={(e) => {
                        e.stopPropagation();
                        void handleDeleteAsset(asset.id);
                      }}
                      className="rounded p-1 text-muted-foreground hover:text-foreground hover:bg-muted cursor-pointer"
                      title="删除资产"
                    >
                      <Trash2 className="h-3.5 w-3.5" />
                    </button>
                  </div>
                );
              })}
            </div>
          )}
        </Card>
      </div>

      {/* Right: 2D Placements / 3D Bindings Editor */}
      <Card className="lg:col-span-7 p-4 space-y-3 text-xs">
        {statusMsg && (
          <div className="rounded border border-xblue/30 bg-xblue-soft/40 px-3 py-1.5">
            {statusMsg}
          </div>
        )}

        {!selectedAsset ? (
          <div className="py-16 text-center text-muted-foreground">
            选择左侧「FLOOR_PLAN_IMAGE」或「MODEL_3D」资产，即可配置 2D 平面摆放坐标或 3D 网格节点绑定。
          </div>
        ) : selectedAsset.asset_kind === "FLOOR_PLAN_IMAGE" ? (
          <div className="space-y-3">
            <div className="flex flex-wrap items-center justify-between gap-2">
              <div>
                <h3 className="font-semibold">
                  2D 平面图摆放编辑器 · {selectedAsset.name}
                </h3>
                <p className="text-[11px] text-muted-foreground">
                  选择业务节点后点击平面图画布放置坐标点，坐标以比例存储
                </p>
              </div>
              <Button
                type="button"
                disabled={savingBindings}
                onClick={() => void handleSavePlacements()}
              >
                <Save className="h-3.5 w-3.5" />
                保存 2D 摆放
              </Button>
            </div>

            <div className="flex items-center gap-2">
              <span className="text-muted-foreground shrink-0">待摆放节点:</span>
              <select
                value={activeNodeId}
                onChange={(e) => setActiveNodeId(e.target.value)}
                className="h-8 flex-1 rounded-md border border-input bg-background px-2 text-xs"
              >
                {nodes.map((n) => (
                  <option key={n.id} value={n.id}>
                    {n.title} ({n.node_key} · {n.type_name})
                  </option>
                ))}
              </select>
            </div>

            <div
              onClick={handleFloorCanvasClick}
              className="relative h-72 w-full rounded border border-border bg-muted/30 overflow-hidden cursor-crosshair"
            >
              {floorImgUrl && (
                <img
                  src={floorImgUrl}
                  alt={selectedAsset.name}
                  className="w-full h-full object-contain pointer-events-none"
                />
              )}
              {placements.map((p) => (
                <div
                  key={p.node_id}
                  style={{
                    left: `${p.x_ratio * 100}%`,
                    top: `${p.y_ratio * 100}%`,
                  }}
                  className="absolute -translate-x-1/2 -translate-y-1/2 rounded border border-xblue bg-background/95 px-1.5 py-0.5 text-[10px] font-mono-tech shadow-xs"
                >
                  {nodeTitleMap.get(p.node_id) ?? p.node_id.slice(0, 8)}
                </div>
              ))}
            </div>
          </div>
        ) : selectedAsset.asset_kind === "MODEL_3D" ? (
          <div className="space-y-3">
            <div className="flex flex-wrap items-center justify-between gap-2">
              <div>
                <h3 className="font-semibold">
                  3D 孪生场景绑定编辑器 · {selectedAsset.name}
                </h3>
                <p className="text-[11px] text-muted-foreground">
                  填写 glTF 命名网格（mesh_name，如 Rack_A）或三维桩点坐标 (pos_x, pos_y, pos_z, scale)
                </p>
              </div>
              <Button
                type="button"
                disabled={savingBindings}
                onClick={() => void handleSaveBindings3d()}
              >
                <Save className="h-3.5 w-3.5" />
                保存 3D 绑定
              </Button>
            </div>

            <div className="flex items-center gap-2">
              <select
                value={activeNodeId}
                onChange={(e) => setActiveNodeId(e.target.value)}
                className="h-8 flex-1 rounded-md border border-input bg-background px-2 text-xs"
              >
                {nodes.map((n) => (
                  <option key={n.id} value={n.id}>
                    {n.title} ({n.node_key} · {n.type_name})
                  </option>
                ))}
              </select>
              <Button type="button" variant="outline" onClick={addBinding3dRow}>
                <Plus className="h-3.5 w-3.5" />
                添加绑定
              </Button>
            </div>

            <div className="space-y-2 max-h-80 overflow-y-auto">
              {bindings3d.map((b, idx) => (
                <div
                  key={b.node_id}
                  className="rounded border border-border p-2.5 grid grid-cols-12 gap-2 items-center"
                >
                  <div className="col-span-4 font-medium truncate">
                    {nodeTitleMap.get(b.node_id) ?? b.node_id}
                  </div>
                  <div className="col-span-3">
                    <Input
                      value={b.mesh_name}
                      onChange={(e) =>
                        setBindings3d((prev) =>
                          prev.map((item, i) =>
                            i === idx ? { ...item, mesh_name: e.target.value } : item
                          )
                        )
                      }
                      placeholder="mesh_name (如 Rack_A)"
                      className="h-7 text-[11px] font-mono-tech"
                    />
                  </div>
                  <div className="col-span-1">
                    <Input
                      type="number"
                      step="0.5"
                      value={b.pos_x}
                      onChange={(e) =>
                        setBindings3d((prev) =>
                          prev.map((item, i) =>
                            i === idx
                              ? { ...item, pos_x: Number(e.target.value) || 0 }
                              : item
                          )
                        )
                      }
                      title="pos_x"
                      className="h-7 text-[11px] font-mono-tech px-1.5"
                    />
                  </div>
                  <div className="col-span-1">
                    <Input
                      type="number"
                      step="0.5"
                      value={b.pos_y}
                      onChange={(e) =>
                        setBindings3d((prev) =>
                          prev.map((item, i) =>
                            i === idx
                              ? { ...item, pos_y: Number(e.target.value) || 0 }
                              : item
                          )
                        )
                      }
                      title="pos_y"
                      className="h-7 text-[11px] font-mono-tech px-1.5"
                    />
                  </div>
                  <div className="col-span-1">
                    <Input
                      type="number"
                      step="0.5"
                      value={b.pos_z}
                      onChange={(e) =>
                        setBindings3d((prev) =>
                          prev.map((item, i) =>
                            i === idx
                              ? { ...item, pos_z: Number(e.target.value) || 0 }
                              : item
                          )
                        )
                      }
                      title="pos_z"
                      className="h-7 text-[11px] font-mono-tech px-1.5"
                    />
                  </div>
                  <div className="col-span-1">
                    <Input
                      type="number"
                      step="0.1"
                      min="0.1"
                      value={b.scale}
                      onChange={(e) =>
                        setBindings3d((prev) =>
                          prev.map((item, i) =>
                            i === idx
                              ? {
                                  ...item,
                                  scale: Math.max(0.1, Number(e.target.value) || 1),
                                }
                              : item
                          )
                        )
                      }
                      title="scale"
                      className="h-7 text-[11px] font-mono-tech px-1.5"
                    />
                  </div>
                  <div className="col-span-1 text-right">
                    <button
                      type="button"
                      onClick={() =>
                        setBindings3d((prev) => prev.filter((_, i) => i !== idx))
                      }
                      className="rounded p-1 text-muted-foreground hover:text-foreground cursor-pointer"
                    >
                      <Trash2 className="h-3.5 w-3.5" />
                    </button>
                  </div>
                </div>
              ))}
            </div>
          </div>
        ) : (
          <div className="py-12 text-center text-muted-foreground">
            {selectedAsset.asset_kind} 资产（{selectedAsset.name}）可在组件编辑器中直接引用。
          </div>
        )}
      </Card>
    </div>
  );
}
