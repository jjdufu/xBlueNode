#!/usr/bin/env node
/**
 * Zero-dependency unit test runner for xbluenode-web using Node 24's built-in
 * `node:test` and native TypeScript type-stripping.
 * Supports optional substring filter arguments, e.g. `npm run test -- xblueview`.
 */
import { spawnSync } from "node:child_process";
import { readdirSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const rootDir = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const srcDir = path.join(rootDir, "src");
const filters = process.argv.slice(2).filter((arg) => arg !== "--" && !arg.startsWith("-"));

function collectTestFiles(dir) {
  const found = [];
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) {
      found.push(...collectTestFiles(full));
    } else if (entry.isFile() && entry.name.endsWith(".test.ts")) {
      found.push(full);
    }
  }
  return found;
}

const allTests = collectTestFiles(srcDir);
const matched =
  filters.length === 0
    ? allTests
    : allTests.filter((f) => {
        const rel = path.relative(rootDir, f).toLowerCase();
        return filters.some((flt) => rel.includes(flt.toLowerCase()));
      });

if (matched.length === 0) {
  console.error(
    `No unit test files matched filter [${filters.join(", ")}] under src/**/*.test.ts`
  );
  process.exit(1);
}

const result = spawnSync(process.execPath, ["--test", ...matched], {
  cwd: rootDir,
  stdio: "inherit",
});
process.exit(result.status ?? 1);
