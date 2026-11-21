import { defineConfig, devices } from "@playwright/test";

declare const process: {
  readonly env: Record<string, string | undefined>;
};

const isCI = Boolean(process.env.CI);
const isLive = process.env.AXPOINT_E2E_LIVE === "1";

export default defineConfig({
  testDir: "./e2e",
  fullyParallel: true,
  forbidOnly: isCI,
  retries: isCI ? 1 : 0,
  workers: 1,
  reporter: "list",
  use: {
    baseURL: "http://127.0.0.1:4173",
    trace: "on-first-retry",
  },
  projects: [
    {
      name: "chromium",
      use: { ...devices["Desktop Chrome"] },
    },
  ],
  webServer: {
    command:
      'VITE_DEV_TOKEN="${VITE_DEV_TOKEN:-${AXPOINT_DEV_TOKEN:-dev-smoke-token-00000000000000000000}}" npm run build && npx vite preview --host 127.0.0.1 --port 4173 --strictPort',
    url: "http://127.0.0.1:4173",
    reuseExistingServer: !isCI && !isLive,
    timeout: 120_000,
  },
});
