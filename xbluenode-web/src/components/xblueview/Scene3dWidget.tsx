import { useEffect, useMemo, useRef, useState } from "react";
import * as THREE from "three";
import { OrbitControls } from "three/examples/jsm/controls/OrbitControls.js";
import { GLTFLoader } from "three/examples/jsm/loaders/GLTFLoader.js";
import { AlertTriangle, Box } from "lucide-react";
import { useI18n } from "@/i18n";
import {
  xblueviewApi,
  type WidgetDataResponse,
  type WidgetRecord,
} from "@/lib/api/xblueview";
import { useTheme } from "@/lib/theme";
import {
  buildColumnIndexMap,
  formatDisplayValue,
  getCell,
  parseThresholds,
  readString,
  resolveThresholdColor,
  toNumericValue,
  type SelectedNodeContext,
} from "./utils";

export interface Scene3dWidgetProps {
  widget: WidgetRecord;
  data: WidgetDataResponse | null;
  isDark?: boolean;
  onSelectNode?: (node: SelectedNodeContext) => void;
  onCrossFilter?: (field: string, value: unknown) => void;
}

export interface Scene3dBindingNode {
  nodeId: string;
  typeName: string;
  nodeKey: string;
  title: string;
  meshName: string;
  posX: number;
  posY: number;
  posZ: number;
  scale: number;
  metricValue: number | null;
  openAlerts: number;
}

interface ProjectedLabel {
  nodeId: string;
  title: string;
  metricValue: number | null;
  openAlerts: number;
  color: string;
  x: number;
  y: number;
  visible: boolean;
}

function canCreateWebGlContext(): boolean {
  if (typeof document === "undefined") return false;
  try {
    const probe = document.createElement("canvas");
    return Boolean(
      probe.getContext("webgl2", { failIfMajorPerformanceCaveat: false }) ||
        probe.getContext("webgl", { failIfMajorPerformanceCaveat: false })
    );
  } catch {
    return false;
  }
}

function disposeMaterialAndTextures(mat: THREE.Material): void {
  const record = mat as unknown as Record<string, unknown>;
  const textureKeys = [
    "map",
    "emissiveMap",
    "normalMap",
    "roughnessMap",
    "metalnessMap",
    "aoMap",
    "alphaMap",
    "bumpMap",
    "lightMap",
  ];
  for (const key of textureKeys) {
    const tex = record[key];
    if (tex instanceof THREE.Texture) {
      tex.dispose();
    }
  }
  mat.dispose();
}

function disposeObjectTree(root: THREE.Object3D): void {
  root.traverse((obj) => {
    if (obj instanceof THREE.Mesh) {
      obj.geometry.dispose();
      if (Array.isArray(obj.material)) {
        for (const m of obj.material) {
          disposeMaterialAndTextures(m);
        }
      } else if (obj.material) {
        disposeMaterialAndTextures(obj.material);
      }
    }
  });
}

export function Scene3dWidget({
  widget,
  data,
  isDark,
  onSelectNode,
  onCrossFilter,
}: Scene3dWidgetProps) {
  const { tx } = useI18n();
  const { isDark: themeDark } = useTheme();
  const effectiveDark = isDark ?? themeDark;

  const containerRef = useRef<HTMLDivElement | null>(null);
  const [webglActive, setWebglActive] = useState<boolean>(() =>
    canCreateWebGlContext()
  );
  const [labels, setLabels] = useState<ProjectedLabel[]>([]);
  const [modelLoaded, setModelLoaded] = useState(false);

  const assetId =
    widget.data_source.kind === "SCENE_3D"
      ? widget.data_source.asset_id
      : readString(widget.options, "asset_id") ?? "";
  const colorMetric =
    widget.data_source.kind === "SCENE_3D"
      ? widget.data_source.color_metric ?? undefined
      : undefined;

  const thresholds = useMemo(
    () => parseThresholds(widget.options),
    [widget.options]
  );

  const nodes = useMemo<Scene3dBindingNode[]>(() => {
    if (!data) return [];
    const colMap = buildColumnIndexMap(data);
    return data.rows.map((r, idx) => {
      const nodeId = String(getCell(r, colMap, "node_id", 0) ?? `node-${idx}`);
      const typeName = String(getCell(r, colMap, "type_name", 1) ?? "Node");
      const nodeKey = String(getCell(r, colMap, "node_key", 2) ?? nodeId);
      const title = String(getCell(r, colMap, "title", 3) ?? nodeKey);
      const meshName = String(getCell(r, colMap, "mesh_name", 4) ?? "");
      const posX = toNumericValue(getCell(r, colMap, "pos_x", 5)) ?? idx * 2.2;
      const posY = toNumericValue(getCell(r, colMap, "pos_y", 6)) ?? 0.6;
      const posZ = toNumericValue(getCell(r, colMap, "pos_z", 7)) ?? 0;
      const scale = Math.max(
        0.1,
        toNumericValue(getCell(r, colMap, "scale", 8)) ?? 1
      );
      const metricValue = toNumericValue(
        getCell(r, colMap, "metric_value", 9)
      );
      const openAlerts =
        toNumericValue(getCell(r, colMap, "open_alerts", 10)) ?? 0;
      return {
        nodeId,
        typeName,
        nodeKey,
        title,
        meshName,
        posX,
        posY,
        posZ,
        scale,
        metricValue,
        openAlerts,
      };
    });
  }, [data]);

  const selectRef = useRef(onSelectNode);
  const crossRef = useRef(onCrossFilter);
  const nodesRef = useRef(nodes);
  const thresholdsRef = useRef(thresholds);
  const colorMetricRef = useRef(colorMetric);
  const syncBindingsFnRef = useRef<(() => void) | null>(null);

  useEffect(() => {
    selectRef.current = onSelectNode;
    crossRef.current = onCrossFilter;
    nodesRef.current = nodes;
    thresholdsRef.current = thresholds;
    colorMetricRef.current = colorMetric;
    syncBindingsFnRef.current?.();
  }, [colorMetric, nodes, onCrossFilter, onSelectNode, thresholds]);

  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;

    if (!canCreateWebGlContext()) {
      setWebglActive(false);
      return;
    }

    let renderer: THREE.WebGLRenderer;
    try {
      renderer = new THREE.WebGLRenderer({
        antialias: true,
        alpha: true,
        powerPreference: "default",
      });
    } catch {
      setWebglActive(false);
      return;
    }

    setWebglActive(true);
    const width = Math.max(container.clientWidth, 240);
    const height = Math.max(container.clientHeight, 180);
    renderer.setPixelRatio(Math.min(window.devicePixelRatio || 1, 2));
    renderer.setSize(width, height);
    renderer.domElement.style.width = "100%";
    renderer.domElement.style.height = "100%";
    renderer.domElement.style.display = "block";
    renderer.domElement.setAttribute("data-testid", "scene3d-canvas");

    container.innerHTML = "";
    container.appendChild(renderer.domElement);

    const scene = new THREE.Scene();
    scene.background = new THREE.Color(effectiveDark ? 0x09090b : 0xf4f4f5);

    const camera = new THREE.PerspectiveCamera(45, width / height, 0.1, 500);
    camera.position.set(6, 5, 8);

    const controls = new OrbitControls(camera, renderer.domElement);
    controls.enableDamping = true;
    controls.dampingFactor = 0.08;

    const ambientLight = new THREE.AmbientLight(0xffffff, 0.8);
    scene.add(ambientLight);
    const dirLight = new THREE.DirectionalLight(0xffffff, 1.1);
    dirLight.position.set(10, 15, 10);
    scene.add(dirLight);

    const gridHelper = new THREE.GridHelper(
      16,
      16,
      effectiveDark ? 0x3f3f46 : 0xd4d4d8,
      effectiveDark ? 0x27272a : 0xe4e4e7
    );
    scene.add(gridHelper);

    const interactiveObjects: THREE.Object3D[] = [];
    const originalMaterials = new Map<
      THREE.Mesh,
      THREE.Material | THREE.Material[]
    >();
    const clonedMaterials: THREE.Material[] = [];
    const nodeAnchors = new Map<
      string,
      {
        obj: THREE.Object3D;
        materials: THREE.MeshStandardMaterial[];
        node: Scene3dBindingNode;
      }
    >();
    const stakesGroup = new THREE.Group();
    scene.add(stakesGroup);

    let disposed = false;
    let gltfGroup: THREE.Object3D | null = null;
    let activeBlobUrl: string | null = null;

    const releaseClonedGltfMaterials = () => {
      for (const [mesh, origMat] of originalMaterials.entries()) {
        mesh.material = origMat;
        delete mesh.userData.nodeId;
      }
      for (const mat of clonedMaterials) {
        mat.dispose();
      }
      clonedMaterials.length = 0;
    };

    const buildOrSyncBindings = () => {
      if (disposed) return;
      releaseClonedGltfMaterials();
      while (stakesGroup.children.length > 0) {
        const child = stakesGroup.children[0];
        if (child) {
          stakesGroup.remove(child);
          disposeObjectTree(child);
        }
      }
      interactiveObjects.length = 0;
      nodeAnchors.clear();

      for (const n of nodesRef.current) {
        const colorHex = resolveThresholdColor(
          n.metricValue,
          thresholdsRef.current,
          "#2563eb"
        );
        const matchedObj =
          gltfGroup && n.meshName
            ? gltfGroup.getObjectByName(n.meshName)
            : undefined;

        const boundMats: THREE.MeshStandardMaterial[] = [];
        if (matchedObj) {
          matchedObj.userData.nodeId = n.nodeId;
          matchedObj.traverse((sub) => {
            if (sub instanceof THREE.Mesh) {
              if (!originalMaterials.has(sub)) {
                originalMaterials.set(sub, sub.material);
              }
              const baseMat = Array.isArray(sub.material)
                ? sub.material[0]
                : sub.material;
              const mat =
                baseMat instanceof THREE.MeshStandardMaterial
                  ? baseMat.clone()
                  : new THREE.MeshStandardMaterial({ color: 0x71717a });
              mat.emissive = new THREE.Color(colorHex);
              mat.emissiveIntensity = n.openAlerts > 0 ? 0.9 : 0.45;
              sub.material = mat;
              sub.userData.nodeId = n.nodeId;
              clonedMaterials.push(mat);
              boundMats.push(mat);
              interactiveObjects.push(sub);
            }
          });
        }

        if (matchedObj && boundMats.length > 0) {
          nodeAnchors.set(n.nodeId, {
            obj: matchedObj,
            materials: boundMats,
            node: n,
          });
        } else {
          const geom = new THREE.BoxGeometry(
            0.9 * n.scale,
            1.2 * n.scale,
            0.9 * n.scale
          );
          const mat = new THREE.MeshStandardMaterial({
            color: effectiveDark ? 0x27272a : 0xe4e4e7,
            emissive: new THREE.Color(colorHex),
            emissiveIntensity: n.openAlerts > 0 ? 0.95 : 0.4,
            roughness: 0.35,
            metalness: 0.25,
          });
          const mesh = new THREE.Mesh(geom, mat);
          mesh.position.set(n.posX, n.posY, n.posZ);
          mesh.userData.nodeId = n.nodeId;
          stakesGroup.add(mesh);
          interactiveObjects.push(mesh);
          nodeAnchors.set(n.nodeId, { obj: mesh, materials: [mat], node: n });
        }
      }
    };

    syncBindingsFnRef.current = buildOrSyncBindings;
    buildOrSyncBindings();

    if (assetId) {
      xblueviewApi
        .fetchAssetBlobUrl(assetId)
        .then(async (blobUrl) => {
          if (disposed) {
            URL.revokeObjectURL(blobUrl);
            return;
          }
          activeBlobUrl = blobUrl;
          const loader = new GLTFLoader();
          const gltf = await loader.loadAsync(blobUrl);
          if (disposed) {
            disposeObjectTree(gltf.scene);
            return;
          }
          gltfGroup = gltf.scene;
          scene.add(gltfGroup);
          setModelLoaded(true);
          buildOrSyncBindings();
        })
        .catch(() => {
          if (!disposed) setModelLoaded(false);
        });
    }

    const raycaster = new THREE.Raycaster();
    const pointer = new THREE.Vector2();

    const onPointerClick = (ev: MouseEvent) => {
      const rect = renderer.domElement.getBoundingClientRect();
      if (rect.width === 0 || rect.height === 0) return;
      pointer.x = ((ev.clientX - rect.left) / rect.width) * 2 - 1;
      pointer.y = -((ev.clientY - rect.top) / rect.height) * 2 + 1;
      raycaster.setFromCamera(pointer, camera);
      const hits = raycaster.intersectObjects(interactiveObjects, true);
      const hit = hits[0];
      if (!hit) return;

      let curr: THREE.Object3D | null = hit.object;
      let hitNodeId: string | undefined;
      while (curr) {
        if (typeof curr.userData.nodeId === "string") {
          hitNodeId = curr.userData.nodeId;
          break;
        }
        curr = curr.parent;
      }
      if (!hitNodeId) return;
      const found = nodesRef.current.find((item) => item.nodeId === hitNodeId);
      if (!found) return;
      selectRef.current?.({
        nodeId: found.nodeId,
        typeName: found.typeName,
        nodeKey: found.nodeKey,
        title: found.title,
        metricName: colorMetricRef.current,
        metricValue: found.metricValue,
      });
      crossRef.current?.("node_key", found.nodeKey);
    };

    renderer.domElement.addEventListener("click", onPointerClick);

    const ro = new ResizeObserver(() => {
      const nextW = Math.max(container.clientWidth, 100);
      const nextH = Math.max(container.clientHeight, 100);
      camera.aspect = nextW / nextH;
      camera.updateProjectionMatrix();
      renderer.setSize(nextW, nextH);
    });
    ro.observe(container);

    let rafId = 0;
    const tmpVec = new THREE.Vector3();
    let frameTick = 0;

    const animate = (nowMs: number) => {
      if (disposed) return;
      rafId = requestAnimationFrame(animate);
      controls.update();

      const pulse = 0.55 + 0.45 * Math.sin(nowMs * 0.006);
      for (const anchor of nodeAnchors.values()) {
        if (anchor.node.openAlerts > 0) {
          for (const mat of anchor.materials) {
            mat.emissiveIntensity = 0.45 + pulse * 0.85;
          }
        }
      }

      renderer.render(scene, camera);

      frameTick += 1;
      if (frameTick % 4 === 1) {
        const cw = renderer.domElement.clientWidth || width;
        const ch = renderer.domElement.clientHeight || height;
        const nextLabels: ProjectedLabel[] = [];
        for (const anchor of nodeAnchors.values()) {
          anchor.obj.getWorldPosition(tmpVec);
          tmpVec.y += 0.9;
          tmpVec.project(camera);
          const visible = tmpVec.z >= -1 && tmpVec.z <= 1;
          nextLabels.push({
            nodeId: anchor.node.nodeId,
            title: anchor.node.title,
            metricValue: anchor.node.metricValue,
            openAlerts: anchor.node.openAlerts,
            color: resolveThresholdColor(
              anchor.node.metricValue,
              thresholdsRef.current,
              "#2563eb"
            ),
            x: (tmpVec.x * 0.5 + 0.5) * cw,
            y: (-tmpVec.y * 0.5 + 0.5) * ch,
            visible,
          });
        }
        setLabels(nextLabels);
      }
    };

    rafId = requestAnimationFrame(animate);

    return () => {
      disposed = true;
      syncBindingsFnRef.current = null;
      cancelAnimationFrame(rafId);
      ro.disconnect();
      renderer.domElement.removeEventListener("click", onPointerClick);
      controls.dispose();
      releaseClonedGltfMaterials();
      originalMaterials.clear();
      if (gltfGroup) {
        disposeObjectTree(gltfGroup);
      }
      disposeObjectTree(scene);
      gridHelper.geometry.dispose();
      if (Array.isArray(gridHelper.material)) {
        for (const m of gridHelper.material) m.dispose();
      } else {
        gridHelper.material.dispose();
      }
      renderer.dispose();
      if (activeBlobUrl) {
        URL.revokeObjectURL(activeBlobUrl);
      }
      if (container.contains(renderer.domElement)) {
        container.removeChild(renderer.domElement);
      }
    };
  }, [assetId, effectiveDark]);

  return (
    <div
      className="h-full relative rounded border border-border overflow-hidden bg-muted/20"
      data-testid={`widget-scene3d-${widget.id}`}
    >
      {webglActive ? (
        <>
          <div ref={containerRef} className="w-full h-full min-h-[180px]" />
          <div className="pointer-events-none absolute inset-0 overflow-hidden">
            {labels.map(
              (lbl) =>
                lbl.visible && (
                  <button
                    key={lbl.nodeId}
                    type="button"
                    onClick={() => {
                      const found = nodes.find((n) => n.nodeId === lbl.nodeId);
                      if (found) {
                        onSelectNode?.({
                          nodeId: found.nodeId,
                          typeName: found.typeName,
                          nodeKey: found.nodeKey,
                          title: found.title,
                          metricName: colorMetric,
                          metricValue: found.metricValue,
                        });
                        onCrossFilter?.("node_key", found.nodeKey);
                      }
                    }}
                    style={{
                      left: `${lbl.x}px`,
                      top: `${lbl.y}px`,
                      borderColor: lbl.color,
                    }}
                    className={`pointer-events-auto absolute -translate-x-1/2 -translate-y-full rounded border bg-background/90 px-1.5 py-0.5 text-[10px] shadow-xs cursor-pointer ${
                      lbl.openAlerts > 0 ? "animate-pulse" : ""
                    }`}
                  >
                    <span className="font-medium">{lbl.title}</span>
                    {lbl.metricValue !== null && (
                      <span className="ml-1 font-mono-tech text-xblue">
                        {formatDisplayValue(lbl.metricValue)}
                      </span>
                    )}
                  </button>
                )
            )}
          </div>
          <div className="absolute left-2 bottom-1.5 rounded bg-background/80 px-1.5 py-0.5 text-[10px] font-mono-tech text-muted-foreground border border-border">
            3D{" "}
            {modelLoaded
              ? tx("glTF 模型", "glTF Model")
              : tx("空间桩点", "Spatial Stakes")}{" "}
            · {tx(`${nodes.length} 节点`, `${nodes.length} nodes`)}
          </div>
        </>
      ) : (
        <div
          className="h-full w-full min-h-[180px] p-3 flex flex-col justify-between bg-muted/30"
          data-testid="scene3d-canvas"
        >
          <div className="flex items-center justify-between text-[11px] text-muted-foreground">
            <span className="inline-flex items-center gap-1.5 font-medium text-foreground">
              <Box className="h-3.5 w-3.5 text-xblue" />
              {tx("3D 空间孪生视图", "3D Spatial Twin")}
            </span>
            <span className="font-mono-tech">
              {tx(`${nodes.length} 绑定对象`, `${nodes.length} bound nodes`)}
            </span>
          </div>

          <div className="grid grid-cols-1 sm:grid-cols-2 gap-2 my-2 overflow-y-auto">
            {nodes.length === 0 ? (
              <div className="col-span-full text-center py-6 text-xs text-muted-foreground">
                {tx("暂无 3D 绑定节点", "No 3D bound nodes")}
              </div>
            ) : (
              nodes.map((n) => {
                const color = resolveThresholdColor(
                  n.metricValue,
                  thresholds,
                  "#2563eb"
                );
                return (
                  <button
                    key={n.nodeId}
                    type="button"
                    onClick={() => {
                      onSelectNode?.({
                        nodeId: n.nodeId,
                        typeName: n.typeName,
                        nodeKey: n.nodeKey,
                        title: n.title,
                        metricName: colorMetric,
                        metricValue: n.metricValue,
                      });
                      onCrossFilter?.("node_key", n.nodeKey);
                    }}
                    style={{ borderLeftColor: color, borderLeftWidth: "3px" }}
                    className={`rounded border border-border bg-background p-2 text-left hover:bg-muted/50 cursor-pointer ${
                      n.openAlerts > 0 ? "ring-1 ring-xblue" : ""
                    }`}
                  >
                    <div className="flex items-center justify-between gap-1 text-xs">
                      <span className="font-medium truncate">{n.title}</span>
                      {n.openAlerts > 0 && (
                        <AlertTriangle className="h-3 w-3 text-xblue shrink-0" />
                      )}
                    </div>
                    <div className="mt-1 flex items-center justify-between text-[11px] font-mono-tech text-muted-foreground">
                      <span>
                        {n.meshName || `(${n.posX}, ${n.posY}, ${n.posZ})`}
                      </span>
                      <span className="text-foreground font-semibold">
                        {formatDisplayValue(n.metricValue)}
                      </span>
                    </div>
                  </button>
                );
              })
            )}
          </div>
        </div>
      )}
    </div>
  );
}
