import { useCallback, useEffect, useRef, useState } from "react";
import * as THREE from "three";
import { OrbitControls } from "three/examples/jsm/controls/OrbitControls.js";
import { GLTFLoader } from "three/examples/jsm/loaders/GLTFLoader.js";
import {
  Box,
  Crosshair,
  Layers,
  Plus,
  Save,
  Trash2,
  Upload,
} from "lucide-react";
import { useI18n } from "@/i18n";
import type { AssetKind } from "@/lib/api/enums";
import { xblueApi, type BlueNode } from "@/lib/api/xblue";
import {
  xblueviewApi,
  type AssetRecord,
  type FloorPlacementRecord,
  type Scene3dBindingRecord,
} from "@/lib/api/xblueview";
import { usePrincipal } from "@/lib/permissions";
import {
  Badge,
  Button,
  Card,
  ConfirmDialog,
  IconButton,
  Input,
  Select,
} from "@/components/ui/primitives";

const ASSET_KINDS: readonly AssetKind[] = [
  "FLOOR_PLAN_IMAGE",
  "MODEL_3D",
  "GEOJSON",
  "ICON",
];

function hasWebGl(): boolean {
  if (typeof document === "undefined") return false;
  try {
    const c = document.createElement("canvas");
    return Boolean(c.getContext("webgl2") || c.getContext("webgl"));
  } catch {
    return false;
  }
}

function disposeThreeTree(root: THREE.Object3D): void {
  root.traverse((obj) => {
    if (obj instanceof THREE.Mesh) {
      obj.geometry.dispose();
      if (Array.isArray(obj.material)) {
        for (const m of obj.material) m.dispose();
      } else if (obj.material) {
        obj.material.dispose();
      }
    }
  });
}

export function AssetManagerPanel() {
  const { tx, tEnum, fmtNum } = useI18n();
  const { can } = usePrincipal();
  const canEdit = can("VIEW_EDIT");

  const [assets, setAssets] = useState<AssetRecord[]>([]);
  const [nodes, setNodes] = useState<BlueNode[]>([]);
  const [selectedAsset, setSelectedAsset] = useState<AssetRecord | null>(null);
  const [pendingDeleteAsset, setPendingDeleteAsset] =
    useState<AssetRecord | null>(null);
  const [deletingAsset, setDeletingAsset] = useState(false);
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
  const floorCanvasRef = useRef<HTMLDivElement | null>(null);
  const [draggingPlacementNodeId, setDraggingPlacementNodeId] = useState<
    string | null
  >(null);
  const didDragPlacementRef = useRef(false);

  // 3D bindings editor + GLTFLoader mesh extraction + Raycaster preview
  const [bindings3d, setBindings3d] = useState<
    Omit<Scene3dBindingRecord, "asset_id">[]
  >([]);
  const [activeBindingIdx, setActiveBindingIdx] = useState<number>(0);
  const [meshNames, setMeshNames] = useState<string[]>([]);
  const [preview3dWebgl, setPreview3dWebgl] = useState<boolean>(() =>
    hasWebGl()
  );
  const preview3dContainerRef = useRef<HTMLDivElement | null>(null);
  const activeBindingIdxRef = useRef<number>(0);

  const [savingBindings, setSavingBindings] = useState(false);
  const [statusMsg, setStatusMsg] = useState<string | null>(null);

  useEffect(() => {
    activeBindingIdxRef.current = activeBindingIdx;
  }, [activeBindingIdx]);

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
      setMeshNames([]);
      setFloorImgUrl(null);
      return;
    }

    let cancelled = false;
    let revokedUrl: string | null = null;

    if (selectedAsset.asset_kind === "FLOOR_PLAN_IMAGE") {
      setMeshNames([]);
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
      setFloorImgUrl(null);
      setActiveBindingIdx(0);
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

  // Load glTF asset with GLTFLoader to extract named Mesh / Object3D names and mount Raycaster preview
  useEffect(() => {
    if (!selectedAsset || selectedAsset.asset_kind !== "MODEL_3D") {
      setMeshNames([]);
      return;
    }

    let disposed = false;
    let activeBlobUrl: string | null = null;
    let gltfRoot: THREE.Object3D | null = null;
    let renderer: THREE.WebGLRenderer | null = null;
    let controls: OrbitControls | null = null;
    let rafId = 0;
    let ro: ResizeObserver | null = null;

    const container = preview3dContainerRef.current;
    const canWebgl = Boolean(container && hasWebGl());
    setPreview3dWebgl(canWebgl);

    const scene = new THREE.Scene();
    scene.background = new THREE.Color(0x18181b);
    const camera = new THREE.PerspectiveCamera(45, 16 / 9, 0.1, 500);
    camera.position.set(5, 4, 6);

    const ambient = new THREE.AmbientLight(0xffffff, 0.9);
    const dirLight = new THREE.DirectionalLight(0xffffff, 1.1);
    dirLight.position.set(8, 12, 8);
    const grid = new THREE.GridHelper(14, 14, 0x3f3f46, 0x27272a);
    scene.add(ambient, dirLight, grid);

    const pickMarker = new THREE.Mesh(
      new THREE.SphereGeometry(0.16, 16, 16),
      new THREE.MeshStandardMaterial({
        color: 0x2563eb,
        emissive: new THREE.Color(0x2563eb),
        emissiveIntensity: 0.7,
      })
    );
    pickMarker.visible = false;
    scene.add(pickMarker);

    const raycaster = new THREE.Raycaster();
    const pointer = new THREE.Vector2();
    const groundPlane = new THREE.Plane(new THREE.Vector3(0, 1, 0), 0);
    const hitVec = new THREE.Vector3();

    const onCanvasClick = (ev: MouseEvent) => {
      if (!renderer || !canEdit) return;
      const rect = renderer.domElement.getBoundingClientRect();
      if (rect.width === 0 || rect.height === 0) return;
      pointer.x = ((ev.clientX - rect.left) / rect.width) * 2 - 1;
      pointer.y = -((ev.clientY - rect.top) / rect.height) * 2 + 1;
      raycaster.setFromCamera(pointer, camera);

      let pickedPoint: THREE.Vector3 | null = null;
      let hitMeshName = "";

      if (gltfRoot) {
        const hits = raycaster.intersectObject(gltfRoot, true);
        const firstHit = hits[0];
        if (firstHit) {
          pickedPoint = firstHit.point.clone();
          let curr: THREE.Object3D | null = firstHit.object;
          while (curr && curr !== gltfRoot) {
            if (curr.name && curr.name.trim().length > 0) {
              hitMeshName = curr.name.trim();
              break;
            }
            curr = curr.parent;
          }
        }
      }

      if (!pickedPoint && raycaster.ray.intersectPlane(groundPlane, hitVec)) {
        pickedPoint = hitVec.clone();
      }

      if (!pickedPoint) return;

      const px = Number(pickedPoint.x.toFixed(2));
      const py = Number(pickedPoint.y.toFixed(2));
      const pz = Number(pickedPoint.z.toFixed(2));
      pickMarker.position.set(px, py, pz);
      pickMarker.visible = true;

      setBindings3d((prev) => {
        if (prev.length === 0) return prev;
        const targetIdx = Math.max(
          0,
          Math.min(prev.length - 1, activeBindingIdxRef.current)
        );
        return prev.map((item, i) =>
          i === targetIdx
            ? {
                ...item,
                pos_x: px,
                pos_y: py,
                pos_z: pz,
                mesh_name: item.mesh_name || hitMeshName,
              }
            : item
        );
      });
      setStatusMsg(
        tx(
          `已拾取坐标 (${px}, ${py}, ${pz})${
            hitMeshName ? ` · ${hitMeshName}` : ""
          }`,
          `Picked (${px}, ${py}, ${pz})${hitMeshName ? ` · ${hitMeshName}` : ""}`
        )
      );
    };

    if (canWebgl && container) {
      try {
        renderer = new THREE.WebGLRenderer({ antialias: true, alpha: false });
        const w = Math.max(container.clientWidth, 280);
        const h = Math.max(container.clientHeight, 180);
        camera.aspect = w / h;
        camera.updateProjectionMatrix();
        renderer.setPixelRatio(Math.min(window.devicePixelRatio || 1, 2));
        renderer.setSize(w, h);
        renderer.domElement.style.width = "100%";
        renderer.domElement.style.height = "100%";
        renderer.domElement.style.display = "block";
        container.innerHTML = "";
        container.appendChild(renderer.domElement);

        controls = new OrbitControls(camera, renderer.domElement);
        controls.enableDamping = true;
        renderer.domElement.addEventListener("click", onCanvasClick);

        ro = new ResizeObserver(() => {
          if (!renderer || !container) return;
          const nw = Math.max(container.clientWidth, 120);
          const nh = Math.max(container.clientHeight, 120);
          camera.aspect = nw / nh;
          camera.updateProjectionMatrix();
          renderer.setSize(nw, nh);
        });
        ro.observe(container);

        const animate = () => {
          if (disposed || !renderer) return;
          rafId = requestAnimationFrame(animate);
          controls?.update();
          renderer.render(scene, camera);
        };
        rafId = requestAnimationFrame(animate);
      } catch {
        setPreview3dWebgl(false);
      }
    }

    xblueviewApi
      .fetchAssetBlobUrl(selectedAsset.id)
      .then(async (blobUrl) => {
        if (disposed) {
          URL.revokeObjectURL(blobUrl);
          return;
        }
        activeBlobUrl = blobUrl;
        const loader = new GLTFLoader();
        const gltf = await loader.loadAsync(blobUrl);
        if (disposed) {
          disposeThreeTree(gltf.scene);
          return;
        }
        const extracted = new Set<string>();
        gltf.scene.traverse((obj) => {
          if (obj.name && obj.name.trim().length > 0) {
            extracted.add(obj.name.trim());
          }
        });
        setMeshNames(Array.from(extracted).sort());
        gltfRoot = gltf.scene;
        scene.add(gltfRoot);
      })
      .catch(() => {
        if (!disposed) setMeshNames([]);
      });

    return () => {
      disposed = true;
      cancelAnimationFrame(rafId);
      ro?.disconnect();
      if (renderer) {
        renderer.domElement.removeEventListener("click", onCanvasClick);
      }
      controls?.dispose();
      if (gltfRoot) {
        disposeThreeTree(gltfRoot);
      }
      disposeThreeTree(pickMarker);
      grid.geometry.dispose();
      if (Array.isArray(grid.material)) {
        for (const m of grid.material) m.dispose();
      } else {
        grid.material.dispose();
      }
      renderer?.dispose();
      if (activeBlobUrl) {
        URL.revokeObjectURL(activeBlobUrl);
      }
      if (renderer && container && container.contains(renderer.domElement)) {
        container.removeChild(renderer.domElement);
      }
    };
  }, [canEdit, selectedAsset, tx]);

  const handleUpload = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!canEdit) return;
    if (!uploadFile) {
      setStatusMsg(tx("未附加资产文件", "Asset file is required"));
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
      setStatusMsg(
        tx(`已上传「${created.name}」`, `Uploaded "${created.name}"`)
      );
      await loadAssetsAndNodes();
      setSelectedAsset(created);
    } catch (err) {
      setStatusMsg(err instanceof Error ? err.message : String(err));
    } finally {
      setUploading(false);
    }
  };

  const confirmDeleteAsset = async () => {
    if (!pendingDeleteAsset || !canEdit) return;
    setDeletingAsset(true);
    try {
      await xblueviewApi.deleteAsset(pendingDeleteAsset.id);
      if (selectedAsset?.id === pendingDeleteAsset.id) {
        setSelectedAsset(null);
      }
      setPendingDeleteAsset(null);
      await loadAssetsAndNodes();
    } catch (err) {
      setStatusMsg(err instanceof Error ? err.message : String(err));
    } finally {
      setDeletingAsset(false);
    }
  };

  const computeFloorRatio = (
    clientX: number,
    clientY: number
  ): { xRatio: number; yRatio: number } | null => {
    const el = floorCanvasRef.current;
    if (!el) return null;
    const rect = el.getBoundingClientRect();
    if (rect.width === 0 || rect.height === 0) return null;
    const xRatio = Number(
      Math.max(0.02, Math.min(0.98, (clientX - rect.left) / rect.width)).toFixed(
        4
      )
    );
    const yRatio = Number(
      Math.max(0.02, Math.min(0.98, (clientY - rect.top) / rect.height)).toFixed(
        4
      )
    );
    return { xRatio, yRatio };
  };

  const handleFloorCanvasClick = (e: React.MouseEvent<HTMLDivElement>) => {
    if (!canEdit) return;
    if (didDragPlacementRef.current) {
      didDragPlacementRef.current = false;
      return;
    }
    if (!activeNodeId) return;
    const pos = computeFloorRatio(e.clientX, e.clientY);
    if (!pos) return;
    setPlacements((prev) => {
      const filtered = prev.filter((p) => p.node_id !== activeNodeId);
      return [
        ...filtered,
        { node_id: activeNodeId, x_ratio: pos.xRatio, y_ratio: pos.yRatio },
      ];
    });
  };

  const handleFloorPointerMove = (e: React.PointerEvent<HTMLDivElement>) => {
    if (!canEdit || !draggingPlacementNodeId) return;
    const pos = computeFloorRatio(e.clientX, e.clientY);
    if (!pos) return;
    didDragPlacementRef.current = true;
    setPlacements((prev) =>
      prev.map((item) =>
        item.node_id === draggingPlacementNodeId
          ? { ...item, x_ratio: pos.xRatio, y_ratio: pos.yRatio }
          : item
      )
    );
  };

  const handleSavePlacements = async () => {
    if (!selectedAsset || !canEdit) return;
    setSavingBindings(true);
    setStatusMsg(null);
    try {
      await xblueviewApi.putFloorPlacements(selectedAsset.id, placements);
      setStatusMsg(tx("平面坐标已保存", "2D placements saved"));
    } catch (err) {
      setStatusMsg(err instanceof Error ? err.message : String(err));
    } finally {
      setSavingBindings(false);
    }
  };

  const handleSaveBindings3d = async () => {
    if (!selectedAsset || !canEdit) return;
    setSavingBindings(true);
    setStatusMsg(null);
    try {
      await xblueviewApi.putScene3dBindings(selectedAsset.id, bindings3d);
      setStatusMsg(tx("三维绑定已保存", "3D bindings saved"));
    } catch (err) {
      setStatusMsg(err instanceof Error ? err.message : String(err));
    } finally {
      setSavingBindings(false);
    }
  };

  const addBinding3dRow = () => {
    if (!activeNodeId || !canEdit) return;
    if (bindings3d.some((b) => b.node_id === activeNodeId)) return;
    setBindings3d((prev) => {
      const next = [
        ...prev,
        {
          node_id: activeNodeId,
          mesh_name: meshNames[0] ?? "",
          pos_x: prev.length * 2,
          pos_y: 0.5,
          pos_z: 0,
          scale: 1,
        },
      ];
      setActiveBindingIdx(next.length - 1);
      return next;
    });
  };

  const nodeTitleMap = new Map(
    nodes.map((n) => [n.id, `${n.title} (${n.node_key})`])
  );
  const datalistId = selectedAsset
    ? `gltf-meshes-${selectedAsset.id}`
    : "gltf-meshes";

  return (
    <div
      className="grid grid-cols-1 lg:grid-cols-12 gap-4"
      data-testid="asset-manager-panel"
    >
      {/* Left: Upload + Asset List */}
      <div className="lg:col-span-5 space-y-4">
        {canEdit && (
          <Card className="p-4 space-y-3">
            <h3 className="text-xs font-semibold flex items-center gap-1.5">
              <Upload className="h-3.5 w-3.5 text-xblue" />
              {tx("上传空间资产", "Upload Spatial Asset")}
            </h3>
            <form
              onSubmit={(e) => void handleUpload(e)}
              className="space-y-2.5 text-xs"
            >
              <div className="grid grid-cols-2 gap-2">
                <div className="space-y-1">
                  <label className="text-muted-foreground block">
                    {tx("资产类型", "Asset Type")}
                  </label>
                  <Select
                    value={uploadKind}
                    onChange={(e) => setUploadKind(e.target.value as AssetKind)}
                    className="h-8 w-full text-xs"
                    data-testid="asset-upload-kind"
                  >
                    {ASSET_KINDS.map((k) => (
                      <option key={k} value={k}>
                        {tEnum(k)} ({k})
                      </option>
                    ))}
                  </Select>
                </div>
                <div className="space-y-1">
                  <label className="text-muted-foreground block">
                    {tx("资产名称", "Asset Name")}
                  </label>
                  <Input
                    value={uploadName}
                    onChange={(e) => setUploadName(e.target.value)}
                    placeholder="warehouse_twin"
                    className="h-8 text-xs"
                    data-testid="asset-upload-name"
                  />
                </div>
              </div>

              <div className="space-y-1">
                <label className="text-muted-foreground block">
                  {tx(
                    "资产文件 (2D/GeoJSON/Icon ≤ 20MB, .glb ≤ 50MB)",
                    "File (2D/GeoJSON/Icon ≤ 20MB, .glb ≤ 50MB)"
                  )}
                </label>
                <label className="flex items-center justify-between gap-2 rounded-md border border-dashed border-border bg-background/60 px-3 py-2 cursor-pointer hover:border-xblue/60 hover:bg-muted/30 transition-colors">
                  <span className="inline-flex items-center gap-2 min-w-0 text-xs">
                    <Upload className="h-3.5 w-3.5 text-xblue shrink-0" />
                    <span className="truncate text-foreground">
                      {uploadFile
                        ? uploadFile.name
                        : tx("选择本地资产文件…", "Choose local asset file…")}
                    </span>
                  </span>
                  <span className="text-[11px] font-medium text-xblue shrink-0">
                    {tx("浏览", "Browse")}
                  </span>
                  <input
                    type="file"
                    onChange={(e) => setUploadFile(e.target.files?.[0] ?? null)}
                    className="sr-only"
                    data-testid="asset-upload-file"
                  />
                </label>
              </div>

              <Button type="submit" disabled={uploading} className="w-full">
                <Upload className="h-3.5 w-3.5" />
                {uploading
                  ? tx("上传中…", "Uploading…")
                  : tx("上传资产", "Upload")}
              </Button>
            </form>
          </Card>
        )}

        <Card className="p-4 space-y-2.5">
          <div className="flex items-center justify-between text-xs">
            <span className="font-semibold">
              {tx(
                `已注册资产 (${fmtNum(assets.length, 0)})`,
                `Registered Assets (${fmtNum(assets.length, 0)})`
              )}
            </span>
            {loading && (
              <span className="text-muted-foreground">
                {tx("加载中…", "Loading…")}
              </span>
            )}
          </div>

          {assets.length === 0 ? (
            <div className="py-6 text-center text-xs text-muted-foreground">
              {tx("暂无空间资产", "No spatial assets")}
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
                        <Badge variant="outline">
                          {tEnum(asset.asset_kind)}
                        </Badge>
                        <span className="font-mono-tech text-[10px] text-muted-foreground">
                          {asset.id.slice(0, 8)}
                        </span>
                      </div>
                    </div>
                    {canEdit && (
                      <IconButton
                        size="sm"
                        onClick={(e) => {
                          e.stopPropagation();
                          setPendingDeleteAsset(asset);
                        }}
                        aria-label={tx("删除资产", "Delete asset")}
                        title={tx("删除资产", "Delete asset")}
                      >
                        <Trash2 className="h-3.5 w-3.5" />
                      </IconButton>
                    )}
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
          <div className="flex flex-col items-center justify-center min-h-[320px] gap-2.5 text-center text-muted-foreground">
            <Layers className="h-8 w-8 text-muted-foreground/45" />
            <p className="max-w-xs text-xs leading-relaxed">
              {tx(
                "从左侧资产列表选定平面图或三维模型以配置节点绑定",
                "Select a 2D floor plan or 3D model on the left to configure node bindings"
              )}
            </p>
          </div>
        ) : selectedAsset.asset_kind === "FLOOR_PLAN_IMAGE" ? (
          <div className="space-y-3">
            <div className="flex flex-wrap items-center justify-between gap-2">
              <div>
                <h3 className="font-semibold">
                  {tx("平面图节点定位", "2D Floor Placement")} ·{" "}
                  {selectedAsset.name}
                </h3>
                <p className="text-[11px] text-muted-foreground">
                  {tx(
                    "在画布定位节点，拖拽已有标记微调比例坐标",
                    "Place nodes on the canvas and drag markers to fine-tune coordinates"
                  )}
                </p>
              </div>
              {canEdit && (
                <Button
                  type="button"
                  size="sm"
                  disabled={savingBindings}
                  onClick={() => void handleSavePlacements()}
                >
                  <Save className="h-3.5 w-3.5" />
                  {tx("保存坐标", "Save")}
                </Button>
              )}
            </div>

            {canEdit && (
              <div className="flex items-center gap-2">
                <span className="text-muted-foreground shrink-0">
                  {tx("目标节点", "Target Node")}:
                </span>
                <Select
                  value={activeNodeId}
                  onChange={(e) => setActiveNodeId(e.target.value)}
                  className="h-8 flex-1 text-xs"
                >
                  {nodes.map((n) => (
                    <option key={n.id} value={n.id}>
                      {n.title} ({n.node_key} · {n.type_name})
                    </option>
                  ))}
                </Select>
              </div>
            )}

            <div
              ref={floorCanvasRef}
              onClick={handleFloorCanvasClick}
              onPointerMove={handleFloorPointerMove}
              onPointerUp={() => setDraggingPlacementNodeId(null)}
              onPointerLeave={() => setDraggingPlacementNodeId(null)}
              className="relative h-72 w-full rounded border border-border bg-muted/30 overflow-hidden cursor-crosshair select-none"
              data-testid="floor-placement-canvas"
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
                  onPointerDown={(e) => {
                    if (!canEdit) return;
                    e.stopPropagation();
                    didDragPlacementRef.current = false;
                    setDraggingPlacementNodeId(p.node_id);
                  }}
                  onClick={(e) => e.stopPropagation()}
                  style={{
                    left: `${p.x_ratio * 100}%`,
                    top: `${p.y_ratio * 100}%`,
                  }}
                  className="absolute -translate-x-1/2 -translate-y-1/2 rounded border border-xblue bg-background/95 px-1.5 py-0.5 text-[10px] font-mono-tech shadow-xs cursor-grab active:cursor-grabbing"
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
                  {tx("三维场景网格绑定", "3D Scene Mesh Binding")} ·{" "}
                  {selectedAsset.name}
                </h3>
                <p className="text-[11px] text-muted-foreground">
                  {tx(
                    "解析 glTF 命名对象并在三维视口拾取表面坐标",
                    "Extracts glTF named meshes with 3D viewport coordinate picking"
                  )}
                </p>
              </div>
              {canEdit && (
                <Button
                  type="button"
                  size="sm"
                  disabled={savingBindings}
                  onClick={() => void handleSaveBindings3d()}
                >
                  <Save className="h-3.5 w-3.5" />
                  {tx("保存绑定", "Save")}
                </Button>
              )}
            </div>

            {/* 3D Raycaster Surface Picker Preview */}
            <div className="rounded border border-border overflow-hidden bg-zinc-950 relative">
              {preview3dWebgl ? (
                <div
                  ref={preview3dContainerRef}
                  className="h-44 w-full cursor-crosshair"
                  data-testid="asset-3d-raycast-canvas"
                />
              ) : (
                <div
                  className="h-24 w-full flex items-center justify-center text-[11px] text-zinc-400 px-3 text-center"
                  data-testid="asset-3d-raycast-fallback"
                >
                  {tx(
                    `当前环境未启用 WebGL 预览，已提取 ${meshNames.length} 个命名对象`,
                    `WebGL preview unavailable; extracted ${meshNames.length} named meshes`
                  )}
                </div>
              )}
              <div className="px-2.5 py-1 bg-zinc-900/90 border-t border-zinc-800 flex items-center justify-between text-[10px] font-mono-tech text-zinc-300">
                <span className="inline-flex items-center gap-1">
                  <Crosshair className="h-3 w-3 text-xblue" />
                  {tx(
                    `视口拾取 (pos_x, pos_y, pos_z) → 第 ${activeBindingIdx + 1} 行`,
                    `Viewport pick (pos_x, pos_y, pos_z) → Row ${activeBindingIdx + 1}`
                  )}
                </span>
                <span>
                  {tx(
                    `命名网格: ${meshNames.length}`,
                    `Meshes: ${meshNames.length}`
                  )}
                </span>
              </div>
            </div>

            <datalist id={datalistId}>
              {meshNames.map((mName) => (
                <option key={mName} value={mName} />
              ))}
            </datalist>

            {canEdit && (
              <div className="flex items-center gap-2">
                <Select
                  value={activeNodeId}
                  onChange={(e) => setActiveNodeId(e.target.value)}
                  className="h-8 flex-1 text-xs"
                >
                  {nodes.map((n) => (
                    <option key={n.id} value={n.id}>
                      {n.title} ({n.node_key} · {n.type_name})
                    </option>
                  ))}
                </Select>
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  onClick={addBinding3dRow}
                >
                  <Plus className="h-3.5 w-3.5" />
                  {tx("添加绑定", "Add Row")}
                </Button>
              </div>
            )}

            <div className="space-y-2 max-h-72 overflow-y-auto">
              {bindings3d.map((b, idx) => {
                const isSelectedRow = idx === activeBindingIdx;
                return (
                  <div
                    key={b.node_id}
                    onClick={() => setActiveBindingIdx(idx)}
                    className={`rounded border p-2.5 grid grid-cols-12 gap-2 items-center cursor-pointer ${
                      isSelectedRow
                        ? "border-xblue bg-xblue-soft/30"
                        : "border-border"
                    }`}
                  >
                    <div className="col-span-3 font-medium truncate">
                      {nodeTitleMap.get(b.node_id) ?? b.node_id}
                    </div>
                    <div className="col-span-4 flex items-center gap-1">
                      {meshNames.length > 0 && (
                        <Select
                          disabled={!canEdit}
                          value={
                            meshNames.includes(b.mesh_name) ? b.mesh_name : ""
                          }
                          onChange={(e) => {
                            const picked = e.target.value;
                            setBindings3d((prev) =>
                              prev.map((item, i) =>
                                i === idx ? { ...item, mesh_name: picked } : item
                              )
                            );
                          }}
                          className="h-7 w-24 shrink-0 px-1 text-[11px] font-mono-tech"
                          title={tx("glTF 命名网格", "glTF mesh")}
                          data-testid={`binding-mesh-select-${idx}`}
                        >
                          <option value="">{tx("未绑定", "Unbound")}</option>
                          {meshNames.map((mName) => (
                            <option key={mName} value={mName}>
                              {mName}
                            </option>
                          ))}
                        </Select>
                      )}
                      <Input
                        list={datalistId}
                        disabled={!canEdit}
                        value={b.mesh_name}
                        onChange={(e) =>
                          setBindings3d((prev) =>
                            prev.map((item, i) =>
                              i === idx
                                ? { ...item, mesh_name: e.target.value }
                                : item
                            )
                          )
                        }
                        placeholder="Rack_A"
                        className="h-7 text-[11px] font-mono-tech flex-1"
                      />
                    </div>
                    <div className="col-span-1">
                      <Input
                        type="number"
                        step="0.5"
                        disabled={!canEdit}
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
                        disabled={!canEdit}
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
                        disabled={!canEdit}
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
                        disabled={!canEdit}
                        value={b.scale}
                        onChange={(e) =>
                          setBindings3d((prev) =>
                            prev.map((item, i) =>
                              i === idx
                                ? {
                                    ...item,
                                    scale: Math.max(
                                      0.1,
                                      Number(e.target.value) || 1
                                    ),
                                  }
                                : item
                            )
                          )
                        }
                        title="scale"
                        className="h-7 text-[11px] font-mono-tech px-1.5"
                      />
                    </div>
                    <div className="col-span-1 flex justify-end">
                      {canEdit && (
                        <IconButton
                          size="sm"
                          onClick={(e) => {
                            e.stopPropagation();
                            setBindings3d((prev) =>
                              prev.filter((_, i) => i !== idx)
                            );
                          }}
                          aria-label={tx("移除绑定", "Remove binding")}
                          title={tx("移除绑定", "Remove binding")}
                        >
                          <Trash2 className="h-3.5 w-3.5" />
                        </IconButton>
                      )}
                    </div>
                  </div>
                );
              })}
            </div>
          </div>
        ) : (
          <div className="py-12 text-center text-muted-foreground">
            {tx(
              `${tEnum(selectedAsset.asset_kind)}「${selectedAsset.name}」可在组件编辑器中绑定`,
              `${selectedAsset.asset_kind} "${selectedAsset.name}" can be bound in Widget Editor`
            )}
          </div>
        )}
      </Card>

      <ConfirmDialog
        open={Boolean(pendingDeleteAsset)}
        onClose={() => setPendingDeleteAsset(null)}
        onConfirm={() => void confirmDeleteAsset()}
        loading={deletingAsset}
        title={tx("删除空间资产", "Delete Spatial Asset")}
        description={
          pendingDeleteAsset
            ? tx(
                `删除资产「${pendingDeleteAsset.name}」后，引用该资产的平面图或三维组件将回退至列表模式。`,
                `Deleting "${pendingDeleteAsset.name}" reverts any bound 2D/3D widgets to list fallback.`
              )
            : ""
        }
      />
    </div>
  );
}
