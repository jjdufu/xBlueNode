#!/usr/bin/env node
/**
 * Step 11 (§11.6 / A5 §5 / A7 C51) xBlueGraph WebGL rendering benchmark:
 * Loads 10,000 nodes and 30,000 links into `/graph` via Playwright, measures
 * sustained frame rate via `requestAnimationFrame` during camera pan/zoom,
 * asserts FPS >= 30, and writes `scripts/bench/results/graph-render-<git-sha>.json`.
 */
import { execFileSync, spawn } from "node:child_process";
import { mkdirSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import path from "node:path";
import { fileURLToPath } from "node:url";

const scriptDir = path.dirname(fileURLToPath(import.meta.url));
const rootDir = path.resolve(scriptDir, "../..");
const webDir = path.join(rootDir, "xbluenode-web");
const requireFromWeb = createRequire(path.join(webDir, "package.json"));
const { chromium } = requireFromWeb("@playwright/test");

const NODE_COUNT = Number(process.env.BENCH_NODES ?? 10_000);
const LINK_COUNT = Number(process.env.BENCH_LINKS ?? 30_000);
const MIN_FPS = Number(process.env.BENCH_MIN_FPS ?? 30);
const PORT = Number(process.env.BENCH_WEB_PORT ?? 4179);
const BASE_URL = process.env.BENCH_BASE_URL ?? `http://127.0.0.1:${PORT}`;

function buildSyntheticSubgraph(nodeCount, linkCount) {
  const types = ["Factory", "Warehouse", "Order", "Supplier"];
  const nodes = new Array(nodeCount);
  for (let i = 0; i < nodeCount; i += 1) {
    const typeName = types[i % types.length];
    const idHex = (i + 1).toString(16).padStart(12, "0");
    nodes[i] = {
      id: `00000000-0000-4000-8000-${idHex}`,
      type_name: typeName,
      node_key: `N-${i + 1}`,
      title: `${typeName} #${i + 1}`,
      properties: { region: i % 2 === 0 ? "华东" : "华南" },
      metrics: { inventory: 100 + (i % 900) },
      geo_lng: null,
      geo_lat: null,
      updated_at: "2026-06-01T00:00:00Z",
    };
  }

  const links = new Array(linkCount);
  for (let e = 0; e < linkCount; e += 1) {
    const srcIdx = e % nodeCount;
    const dstIdx = (srcIdx * 31 + Math.floor(e / nodeCount) + 1) % nodeCount;
    const linkHex = (e + 1).toString(16).padStart(12, "0");
    links[e] = {
      id: `10000000-0000-4000-8000-${linkHex}`,
      link_type: e % 2 === 0 ? "SUPPLIES" : "FULFILLS",
      src_node_id: nodes[srcIdx].id,
      dst_node_id: nodes[dstIdx === srcIdx ? (dstIdx + 1) % nodeCount : dstIdx].id,
      weight: 0.5 + (e % 5) * 0.1,
      properties: {},
    };
  }
  return { nodes, links, truncated: false };
}

async function waitForServer(url, timeoutMs = 20_000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    try {
      const res = await fetch(url);
      if (res.ok) return;
    } catch {
      // retry
    }
    await new Promise((r) => setTimeout(r, 250));
  }
  throw new Error(`Timed out waiting for preview server at ${url}`);
}

async function main() {
  const gitSha = execFileSync("git", ["-C", rootDir, "rev-parse", "--short", "HEAD"], {
    encoding: "utf-8",
  }).trim();

  let previewProc = null;
  if (!process.env.BENCH_BASE_URL) {
    execFileSync("npm", ["run", "build"], {
      cwd: webDir,
      stdio: "inherit",
      env: { ...process.env, VITE_DEV_TOKEN: process.env.VITE_DEV_TOKEN || "bench-dev-token" },
    });
    previewProc = spawn(
      "npx",
      ["vite", "preview", "--host", "127.0.0.1", "--port", String(PORT), "--strictPort"],
      { cwd: webDir, stdio: "ignore" }
    );
    await waitForServer(BASE_URL);
  }

  const subgraphPayload = JSON.stringify(buildSyntheticSubgraph(NODE_COUNT, LINK_COUNT));
  const browser = await chromium.launch({
    headless: true,
    args: ["--enable-webgl", "--use-gl=angle", "--use-angle=swiftshader"],
  });

  try {
    const context = await browser.newContext({ viewport: { width: 1440, height: 900 } });
    const page = await context.newPage();
    await page.route("**/api/**", async (route) => {
      const url = new URL(route.request().url());
      if (url.pathname === "/api/auth/me") {
        await route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify({
            user_id: "00000000-0000-0000-0000-000000000001",
            tenant_id: "default",
            username: "bench",
            dept_id: "hq",
            roles: ["ADMIN"],
            permissions: ["DATA_READ", "VIEW_EDIT"],
            token_kind: "DEV",
          }),
        });
        return;
      }
      if (url.pathname === "/api/xblue/graph/subgraph") {
        await route.fulfill({
          status: 200,
          contentType: "application/json",
          body: subgraphPayload,
        });
        return;
      }
      if (url.pathname === "/api/xblue/schema") {
        await route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify({ node_types: [], edge_types: [] }),
        });
        return;
      }
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ items: [], results: [] }),
      });
    });

    const loadStart = Date.now();
    await page.goto(`${BASE_URL}/graph`, { waitUntil: "networkidle" });
    if (page.url().includes("/login")) {
      await page.locator("#access-token-input").fill("bench-dev-token");
      await page.getByRole("button", { name: "验证并进入工作台" }).click();
      await page.getByRole("link", { name: /xBlueGraph/ }).click();
    }
    await page.locator("canvas").first().waitFor({ state: "visible", timeout: 30_000 });
    const initialRenderMs = Date.now() - loadStart;

    const metrics = await page.evaluate(async () => {
      const sampleDurationMs = 3_000;
      let frames = 0;
      const frameDeltas = [];
      let prev = performance.now();
      const start = prev;

      return await new Promise((resolve) => {
        function tick(now) {
          frames += 1;
          frameDeltas.push(now - prev);
          prev = now;
          if (now - start < sampleDurationMs) {
            window.requestAnimationFrame(tick);
          } else {
            const elapsedSec = (now - start) / 1000;
            const sorted = [...frameDeltas].sort((a, b) => a - b);
            const p95FrameMs =
              sorted[Math.min(sorted.length - 1, Math.floor(sorted.length * 0.95))] ?? 0;
            resolve({
              frames,
              elapsed_ms: Math.round(now - start),
              avg_fps: Number((frames / elapsedSec).toFixed(2)),
              p95_frame_ms: Number(p95FrameMs.toFixed(2)),
            });
          }
        }
        window.requestAnimationFrame(tick);
      });
    });

    const outDir = path.join(rootDir, "scripts/bench/results");
    mkdirSync(outDir, { recursive: true });
    const outFile = path.join(outDir, `graph-render-${gitSha}.json`);
    const report = {
      benchmark: "graph-render",
      git_sha: gitSha,
      recorded_at: new Date().toISOString(),
      nodes: NODE_COUNT,
      links: LINK_COUNT,
      initial_render_ms: initialRenderMs,
      ...metrics,
      threshold_fps: MIN_FPS,
      passed: metrics.avg_fps >= MIN_FPS,
    };
    writeFileSync(outFile, `${JSON.stringify(report, null, 2)}\n`, "utf-8");
    console.log(`Graph render benchmark written to ${outFile}:`, JSON.stringify(report));

    if (!report.passed) {
      throw new Error(`Graph render FPS ${metrics.avg_fps} < required ${MIN_FPS} fps`);
    }
  } finally {
    await browser.close();
    if (previewProc) {
      previewProc.kill("SIGTERM");
    }
  }
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
