import fs from "node:fs";
import readline from "node:readline";
import { Resvg } from "@resvg/resvg-js";
import * as echarts from "echarts";

const writeStdoutLine = (payload) => {
  process.stdout.write(`${JSON.stringify(payload)}\n`);
};

// Prevent ECharts or third-party warnings from polluting stdout JSONL or blocking stderr pipes.
console.log = () => {};
console.info = () => {};
console.warn = () => {};
console.error = () => {};

const CJK_FONT_CANDIDATES = [
  "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
  "/usr/share/fonts/opentype/noto/NotoSansCJKsc-Regular.otf",
  "/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc",
];

const FONT_FILES = CJK_FONT_CANDIDATES.filter((fontPath) => {
  try {
    return fs.statSync(fontPath).isFile();
  } catch {
    return false;
  }
});

const DEFAULT_FONT_FAMILY = "Noto Sans CJK SC, Noto Sans CJK, sans-serif";

const SUSPICIOUS_EXPR_PATTERNS = [
  /^\s*function\s*\(/i,
  /\([^)]*\)\s*=>/,
  /^[A-Za-z_$][A-Za-z0-9_$]*\s*=>/,
  /<script\b/i,
  /\bjavascript\s*:/i,
];

function assertSafeOption(value, keyName = "", depth = 0) {
  if (depth > 32) {
    throw new Error("chart option exceeds maximum nesting depth of 32");
  }
  if (value === null || value === undefined) {
    return;
  }
  const valueType = typeof value;
  if (valueType === "function" || valueType === "symbol") {
    throw new Error("functions are forbidden in chart option");
  }
  if (keyName === "formatter" && valueType !== "string") {
    throw new Error("formatter must be a template string");
  }
  if (valueType === "string") {
    for (const pattern of SUSPICIOUS_EXPR_PATTERNS) {
      if (pattern.test(value)) {
        throw new Error("executable expressions are forbidden in chart option");
      }
    }
    return;
  }
  if (Array.isArray(value)) {
    for (const item of value) {
      assertSafeOption(item, "", depth + 1);
    }
    return;
  }
  if (valueType === "object") {
    for (const [k, v] of Object.entries(value)) {
      if (k === "__proto__" || k === "constructor" || k === "prototype") {
        throw new Error(`forbidden property key in chart option: ${k}`);
      }
      assertSafeOption(v, k, depth + 1);
    }
  }
}

function handleRenderRequest(req) {
  const id = req && typeof req.id === "string" ? req.id : "";
  const width = Number(req?.width ?? 960);
  const height = Number(req?.height ?? 540);
  const format = String(req?.format ?? "png").toLowerCase();
  const rawTheme = req?.theme ?? null;

  if (!Number.isInteger(width) || width < 64 || width > 4096) {
    return {
      id,
      ok: false,
      code: "INVALID_ARGUMENT",
      error: `width must be an integer in 64..=4096, got ${String(req?.width)}`,
    };
  }
  if (!Number.isInteger(height) || height < 64 || height > 4096) {
    return {
      id,
      ok: false,
      code: "INVALID_ARGUMENT",
      error: `height must be an integer in 64..=4096, got ${String(req?.height)}`,
    };
  }
  if (format !== "svg" && format !== "png") {
    return {
      id,
      ok: false,
      code: "INVALID_ARGUMENT",
      error: `unsupported chart format: ${format}`,
    };
  }
  if (
    !req?.option ||
    typeof req.option !== "object" ||
    Array.isArray(req.option)
  ) {
    return {
      id,
      ok: false,
      code: "INVALID_ARGUMENT",
      error: "option must be a JSON object",
    };
  }

  try {
    assertSafeOption(req.option);
  } catch (err) {
    return {
      id,
      ok: false,
      code: "INVALID_ARGUMENT",
      error: err instanceof Error ? err.message : String(err),
    };
  }

  const themeName = rawTheme === "dark" ? "dark" : null;
  const defaultBg = themeName === "dark" ? "#0f172a" : "#ffffff";
  const bg = req.option.backgroundColor ?? defaultBg;
  const baseTextStyle =
    req.option.textStyle &&
    typeof req.option.textStyle === "object" &&
    !Array.isArray(req.option.textStyle)
      ? req.option.textStyle
      : {};

  const mergedOption = {
    ...req.option,
    backgroundColor: bg,
    animation: false,
    textStyle: {
      fontFamily: DEFAULT_FONT_FAMILY,
      ...baseTextStyle,
    },
  };

  let chart = null;
  try {
    chart = echarts.init(null, themeName, {
      renderer: "svg",
      ssr: true,
      width,
      height,
    });
    chart.setOption(mergedOption);
    const svgString = chart.renderToSVGString();

    if (format === "svg") {
      return {
        id,
        ok: true,
        data_b64: Buffer.from(svgString, "utf-8").toString("base64"),
        mime: "image/svg+xml",
      };
    }

    const resvg = new Resvg(svgString, {
      fitTo: { mode: "width", value: width * 2 },
      background: typeof bg === "string" ? bg : defaultBg,
      font: {
        fontFiles: FONT_FILES,
        loadSystemFonts: FONT_FILES.length === 0,
        defaultFontFamily: "Noto Sans CJK SC",
        sansSerifFamily: "Noto Sans CJK SC",
      },
    });
    const rendered = resvg.render();
    const pngBytes = rendered.asPng();
    return {
      id,
      ok: true,
      data_b64: Buffer.from(pngBytes).toString("base64"),
      mime: "image/png",
    };
  } catch (err) {
    return {
      id,
      ok: false,
      code: "UPSTREAM_PARSER",
      error: err instanceof Error ? err.message : String(err),
    };
  } finally {
    if (chart) {
      try {
        chart.dispose();
      } catch {
        // Ignore dispose cleanup error
      }
    }
  }
}

const rl = readline.createInterface({
  input: process.stdin,
  crlfDelay: Infinity,
  terminal: false,
});

rl.on("line", (line) => {
  const trimmed = line.trim();
  if (!trimmed) {
    return;
  }
  let parsed;
  try {
    parsed = JSON.parse(trimmed);
  } catch (err) {
    writeStdoutLine({
      id: "",
      ok: false,
      code: "INVALID_ARGUMENT",
      error: `invalid JSON request: ${err instanceof Error ? err.message : String(err)}`,
    });
    return;
  }
  const response = handleRenderRequest(parsed);
  writeStdoutLine(response);
});
