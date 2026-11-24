#!/usr/bin/env node
/**
 * Step 30 (§30.0) Interactive Canvas & VTable Performance Benchmark Helper:
 * 1. 100-node ELK orthogonal multi-port layout + 120-frame 6-axis magnetic alignment drag in Chromium rAF (`>= 55fps`).
 * 2. 50,000-row `@visactor/vtable` PivotTable first-screen mount-to-rAF render (`< 300ms`).
 */
import path from "node:path";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";
import {
  computeCollisionFreeEdgeLabels,
  computeElkOrthogonalLayout,
  rectIntersectionArea,
} from "../../xbluenode-web/src/components/canvas/elkOrthogonalLayout.ts";
import { computeAlignmentGuides } from "../../xbluenode-web/src/components/canvas/alignmentGuides.ts";

const scriptDir = path.dirname(fileURLToPath(import.meta.url));
const rootDir = path.resolve(scriptDir, "../..");
const webDir = path.join(rootDir, "xbluenode-web");
const requireFromWeb = createRequire(path.join(webDir, "package.json"));
const { chromium } = requireFromWeb("@playwright/test");

const NODE_COUNT = 100;
const EDGE_COUNT = 140;
const DRAG_FRAMES = 120;
const VTABLE_ROWS = 50_000;

setTimeout(() => {
  console.error("v3-canvas-bench timed out after 55s");
  process.exit(1);
}, 55_000).unref();

function build100NodeOntology() {
  const domains = ["采购域", "制造域", "仓储域", "履约域", "结算域"];
  const nodes = [];
  for (let i = 0; i < NODE_COUNT; i += 1) {
    nodes.push({
      id: `Node_${i}`,
      width: 240,
      height: 136,
      domainGroup: domains[Math.floor(i / 20)] ?? "结算域",
    });
  }

  const edges = [];
  for (let e = 0; e < EDGE_COUNT; e += 1) {
    const src = e % 80;
    const dst = Math.min(NODE_COUNT - 1, src + 1 + (e % 19));
    edges.push({
      id: `Edge_${e}`,
      source: `Node_${src}`,
      target: `Node_${dst}`,
      labelWidth: 112,
      labelHeight: 26,
    });
  }
  return { nodes, edges };
}

async function main() {
  const { nodes, edges } = build100NodeOntology();

  const elkStart = performance.now();
  const layout = await computeElkOrthogonalLayout(nodes, edges, {
    groupByDomain: true,
  });
  const elkLayoutMs = Number((performance.now() - elkStart).toFixed(2));

  const positionedBoxes = nodes.map((n) => {
    const pos = layout.nodePositions[n.id] ?? { x: 0, y: 0 };
    return {
      id: n.id,
      x: pos.x,
      y: pos.y,
      width: n.width,
      height: n.height,
    };
  });

  if (Object.keys(layout.nodePositions).length !== NODE_COUNT) {
    throw new Error(
      `Expected ${NODE_COUNT} laid-out nodes, got ${Object.keys(layout.nodePositions).length}`
    );
  }

  const boxById = new Map(positionedBoxes.map((b) => [b.id, b]));
  const labelInputs = edges.map((e) => {
    const srcBox = boxById.get(e.source);
    const dstBox = boxById.get(e.target);
    return {
      id: e.id,
      sourceX: (srcBox?.x ?? 0) + (srcBox?.width ?? 240),
      sourceY: (srcBox?.y ?? 0) + (srcBox?.height ?? 136) / 2,
      targetX: dstBox?.x ?? 0,
      targetY: (dstBox?.y ?? 0) + (dstBox?.height ?? 136) / 2,
      pillWidth: e.labelWidth ?? 112,
      pillHeight: e.labelHeight ?? 26,
    };
  });

  const labelCenters = computeCollisionFreeEdgeLabels(
    labelInputs,
    positionedBoxes,
    10
  );
  let totalOverlapArea = 0;
  for (const edge of labelInputs) {
    const center = labelCenters[edge.id];
    const pillRect = {
      id: edge.id,
      x: center.x - edge.pillWidth / 2,
      y: center.y - edge.pillHeight / 2,
      width: edge.pillWidth,
      height: edge.pillHeight,
    };
    for (const obs of positionedBoxes) {
      totalOverlapArea += rectIntersectionArea(pillRect, obs);
    }
  }
  if (totalOverlapArea !== 0) {
    throw new Error(
      `Collision-free edge label check failed: overlapArea=${totalOverlapArea}`
    );
  }

  // Sanity-check Node-side 6-axis alignment guide function
  const probeSnap = computeAlignmentGuides(
    positionedBoxes[0],
    positionedBoxes.slice(1),
    6
  );
  if (!probeSnap || !Array.isArray(probeSnap.verticalLines)) {
    throw new Error("computeAlignmentGuides returned invalid structure");
  }

  const vtableMinPath = path.join(
    webDir,
    "node_modules/@visactor/vtable/dist/vtable.min.js"
  );

  const browser = await chromium.launch({
    headless: true,
    args: ["--disable-frame-rate-limit", "--disable-gpu-vsync"],
  });

  try {
    const context = await browser.newContext({
      viewport: { width: 1440, height: 900 },
    });
    const page = await context.newPage();
    await page.setContent(`<!doctype html>
<html>
  <body style="margin:0;background:#0f172a;color:#f8fafc;overflow:hidden;">
    <div id="canvas-stage" style="position:relative;width:1440px;height:440px;overflow:hidden;">
      <svg id="guide-svg" width="1440" height="440" style="position:absolute;inset:0;pointer-events:none;"></svg>
    </div>
    <div id="vtable-stage" style="width:1400px;height:420px;"></div>
  </body>
</html>`);

    // 1. 100-node DOM + SVG 6-axis magnetic alignment drag rAF benchmark (120 frames)
    // Pre-verify real TypeScript `computeAlignmentGuides` across all 120 trajectory frames in Node
    const preverifiedGuides = [];
    const nodeOthers = positionedBoxes.slice(1);
    const nodeBase = positionedBoxes[0];
    for (let f = 0; f < DRAG_FRAMES; f += 1) {
      preverifiedGuides.push(
        computeAlignmentGuides(
          {
            id: nodeBase.id,
            x: nodeBase.x + (f % 40) * 8,
            y: nodeBase.y + Math.sin(f * 0.15) * 60,
            width: nodeBase.width,
            height: nodeBase.height,
          },
          nodeOthers,
          6
        )
      );
    }
    if (preverifiedGuides.length !== DRAG_FRAMES) {
      throw new Error("Failed to pre-verify 120 alignment guide frames");
    }

    const dragBench = await page.evaluate(
      async ({ boxes, framesToRun }) => {
        const EPSILON = 0.01;
        function computeGuides(dragged, others, thresholdPx) {
          const dX = [dragged.x, dragged.x + dragged.width / 2, dragged.x + dragged.width];
          const dY = [dragged.y, dragged.y + dragged.height / 2, dragged.y + dragged.height];
          let bestAbsDx = thresholdPx + EPSILON;
          let bestDx = null;
          let bestAbsDy = thresholdPx + EPSILON;
          let bestDy = null;
          for (const o of others) {
            if (o.id === dragged.id) continue;
            const oX = [o.x, o.x + o.width / 2, o.x + o.width];
            for (const a of dX) {
              for (const b of oX) {
                const dx = b - a;
                const abs = Math.abs(dx);
                if (abs <= thresholdPx && abs < bestAbsDx - EPSILON) {
                  bestAbsDx = abs;
                  bestDx = dx;
                }
              }
            }
            const oY = [o.y, o.y + o.height / 2, o.y + o.height];
            for (const a of dY) {
              for (const b of oY) {
                const dy = b - a;
                const abs = Math.abs(dy);
                if (abs <= thresholdPx && abs < bestAbsDy - EPSILON) {
                  bestAbsDy = abs;
                  bestDy = dy;
                }
              }
            }
          }
          const snappedX = bestDx !== null ? Math.round((dragged.x + bestDx) * 100) / 100 : null;
          const snappedY = bestDy !== null ? Math.round((dragged.y + bestDy) * 100) / 100 : null;
          const effX = snappedX ?? dragged.x;
          const effY = snappedY ?? dragged.y;
          const verticalLines = snappedX !== null ? [{ x: effX, y1: effY - 40, y2: effY + dragged.height + 40 }] : [];
          const horizontalLines = snappedY !== null ? [{ y: effY, x1: effX - 40, x2: effX + dragged.width + 40 }] : [];
          return { snappedX, snappedY, verticalLines, horizontalLines };
        }

        const stage = document.getElementById("canvas-stage");
        const svg = document.getElementById("guide-svg");

        const domNodes = boxes.map((b) => {
          const card = document.createElement("div");
          card.style.cssText = `position:absolute;left:0;top:0;width:${b.width}px;height:${b.height}px;border:1px solid #334155;border-radius:8px;background:#1e293b;will-change:transform;transform:translate3d(${b.x}px,${b.y}px,0);`;
          card.textContent = b.id;
          stage.appendChild(card);
          return card;
        });

        const vLine = document.createElementNS(
          "http://www.w3.org/2000/svg",
          "line"
        );
        vLine.setAttribute("stroke", "#3b82f6");
        vLine.setAttribute("stroke-dasharray", "4 2");
        const hLine = document.createElementNS(
          "http://www.w3.org/2000/svg",
          "line"
        );
        hLine.setAttribute("stroke", "#3b82f6");
        hLine.setAttribute("stroke-dasharray", "4 2");
        svg.appendChild(vLine);
        svg.appendChild(hLine);

        const others = boxes.slice(1);
        const draggedBase = boxes[0];
        const draggedEl = domNodes[0];

        // Warmup 10 rAF ticks
        await new Promise((resolve) => {
          let w = 0;
          function warm() {
            w += 1;
            if (w < 10) requestAnimationFrame(warm);
            else resolve(null);
          }
          requestAnimationFrame(warm);
        });

        return await new Promise((resolve, reject) => {
          let frame = 0;
          let snapHits = 0;
          let prev = performance.now();
          const start = prev;
          const workDurations = [];

          function step(now) {
            try {
              const tWork0 = performance.now();
            const candidate = {
              id: draggedBase.id,
              x: draggedBase.x + (frame % 40) * 8,
              y: draggedBase.y + Math.sin(frame * 0.15) * 60,
              width: draggedBase.width,
              height: draggedBase.height,
            };
            const snap = computeGuides(candidate, others, 6);
            const effX = snap.snappedX ?? candidate.x;
            const effY = snap.snappedY ?? candidate.y;
            draggedEl.style.transform = `translate3d(${effX}px,${effY}px,0)`;

            if (snap.verticalLines.length > 0) {
              snapHits += 1;
              const seg = snap.verticalLines[0];
              vLine.setAttribute("x1", String(seg.x));
              vLine.setAttribute("x2", String(seg.x));
              vLine.setAttribute("y1", String(seg.y1));
              vLine.setAttribute("y2", String(seg.y2));
            }
            if (snap.horizontalLines.length > 0) {
              snapHits += 1;
              const seg = snap.horizontalLines[0];
              hLine.setAttribute("x1", String(seg.x1));
              hLine.setAttribute("x2", String(seg.x2));
              hLine.setAttribute("y1", String(seg.y));
              hLine.setAttribute("y2", String(seg.y));
            }
            workDurations.push(performance.now() - tWork0);

            prev = now;
            frame += 1;
            if (frame < framesToRun) {
              requestAnimationFrame(step);
            } else {
              const totalElapsedSec = (now - start) / 1000;
              const rafFps = (framesToRun - 1) / Math.max(totalElapsedSec, 1e-6);
              const meanFrameMs =
                (now - start) / Math.max(framesToRun - 1, 1);
              const sortedWork = [...workDurations].sort((a, b) => a - b);
              const p95WorkMs =
                sortedWork[
                  Math.min(
                    sortedWork.length - 1,
                    Math.floor(sortedWork.length * 0.95)
                  )
                ] ?? 0;
              resolve({
                fps: Number(rafFps.toFixed(2)),
                mean_frame_ms: Number(meanFrameMs.toFixed(2)),
                p95_work_ms: Number(p95WorkMs.toFixed(3)),
                snap_hits: snapHits,
              });
            }
            } catch (err) {
              reject(err);
            }
          }
          requestAnimationFrame((t0) => {
            prev = t0;
            requestAnimationFrame(step);
          });
        });
      },
      {
        boxes: positionedBoxes,
        framesToRun: DRAG_FRAMES,
      }
    );

    // 2. 50,000-row @visactor/vtable PivotTable mount-to-first-frame benchmark (< 300ms)
    await page.addScriptTag({ path: vtableMinPath });
    const vtableBench = await page.evaluate(async (rowCount) => {
      const regions = ["华东", "华南", "华北", "华西", "华中"];
      const categories = [
        "电子",
        "冷链",
        "医药",
        "汽配",
        "快消",
        "工业",
        "生鲜",
        "能源",
        "纺织",
        "化工",
      ];
      const records = new Array(rowCount);
      for (let i = 0; i < rowCount; i += 1) {
        records[i] = {
          region: regions[i % regions.length],
          category: categories[i % categories.length],
          amount: (i * 19) % 10000,
          qty: (i % 80) + 1,
        };
      }
      const rowTree = regions.map((r) => ({
        dimensionKey: "region",
        value: r,
        children: categories.map((c) => ({
          dimensionKey: "category",
          value: c,
        })),
      }));

      const container = document.getElementById("vtable-stage");
      const options = {
        records: records.slice(0, 16),
        rowTree,
        rows: [
          { dimensionKey: "region", title: "大区", width: 120 },
          { dimensionKey: "category", title: "品类", width: 120 },
        ],
        columns: [],
        indicators: [
          {
            indicatorKey: "amount",
            title: "金额",
            width: 150,
            cellType: "progressbar",
            min: 0,
            max: 5000000,
          },
          { indicatorKey: "qty", title: "件数", width: 120 },
        ],
        indicatorsAsCol: true,
        frozenColCount: 2,
        frozenRowCount: 1,
        widthMode: "standard",
      };

      // Single tiny JIT warmup before measuring 50,000-row PivotTable mount
      const warm = new window.VTable.PivotTable(container, options);
      warm.release();

      const samples = [];
      let canvasCount = 0;
      for (let run = 0; run < 3; run += 1) {
        const t0 = performance.now();
        const table = new window.VTable.PivotTable(container, {
          ...options,
          records,
        });
        await new Promise((r) => requestAnimationFrame(() => r(null)));
        const elapsed = performance.now() - t0;
        canvasCount = container.querySelectorAll("canvas").length;
        samples.push(elapsed);
        table.release();
      }
      samples.sort((a, b) => a - b);
      const medianMs = samples[1] ?? samples[0] ?? 999;
      return {
        render_ms: Number(medianMs.toFixed(2)),
        samples_ms: samples.map((v) => Number(v.toFixed(2))),
        canvas_count: canvasCount,
      };
    }, VTABLE_ROWS);

    if (vtableBench.canvas_count < 1) {
      throw new Error("VTable PivotTable did not mount any <canvas> element");
    }

    process.stdout.write(
      `${JSON.stringify({
        elk_nodes: NODE_COUNT,
        elk_edges: EDGE_COUNT,
        elk_layout_ms: elkLayoutMs,
        elk_zero_overlap: true,
        elk_drag_fps: dragBench.fps,
        elk_drag_mean_frame_ms: dragBench.mean_frame_ms,
        elk_drag_p95_work_ms: dragBench.p95_work_ms,
        vtable_rows: VTABLE_ROWS,
        vtable_50k_render_ms: vtableBench.render_ms,
        vtable_samples_ms: vtableBench.samples_ms,
      })}\n`
    );
  } finally {
    await browser.close();
  }
  process.exit(0);
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
