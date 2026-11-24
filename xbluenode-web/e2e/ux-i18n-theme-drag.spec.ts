import { expect, test } from "@playwright/test";

test.describe("UX overhaul: i18n, system theme persistence, topbar layout & canvas drag", () => {
  test("follows browser en-US locale & OS dark scheme by default, toggles & persists across reload", async ({
    browser,
  }) => {
    const context = await browser.newContext({
      locale: "en-US",
      colorScheme: "dark",
    });
    const page = await context.newPage();

    await page.route("**/api/auth/config", async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          env: "development",
          oidc_enabled: false,
          default_tenant: "default",
        }),
      });
    });

    await page.goto("/login");

    // 1. Auto-detected dark mode from OS prefers-color-scheme: dark
    await expect(page.locator("html")).toHaveClass(/dark/);
    // 2. Auto-detected English from browser locale en-US
    await expect(
      page.getByRole("heading", { level: 1, name: "Sign In" })
    ).toBeVisible();

    // 3. Theme toggle is a borderless icon-only button in top-right
    const themeBtn = page.getByTestId("theme-toggle-btn");
    const themeBox = await themeBtn.boundingBox();
    expect(themeBox).not.toBeNull();
    expect(themeBox!.y).toBeLessThan(60);
    expect(themeBox!.x).toBeGreaterThan(600);
    expect((await themeBtn.innerText()).trim()).toBe("");

    // Click to switch to light mode & verify persistence across reload
    await themeBtn.click();
    await expect(page.locator("html")).not.toHaveClass(/dark/);
    await page.reload();
    await expect(page.locator("html")).not.toHaveClass(/dark/);

    // 4. Top-right locale switcher toggles en-US -> zh-CN and persists across reload
    const localeBtn = page.getByTestId("locale-toggle-btn");
    const localeBox = await localeBtn.boundingBox();
    expect(localeBox).not.toBeNull();
    expect(localeBox!.y).toBeLessThan(60);
    expect(localeBox!.x).toBeGreaterThan(600);

    await localeBtn.click();
    await expect(
      page.getByRole("heading", { level: 1, name: "登录" })
    ).toBeVisible();
    await page.reload();
    await expect(
      page.getByRole("heading", { level: 1, name: "登录" })
    ).toBeVisible();

    await context.close();
  });

  test("authenticated topbar keeps logout in top-right and StudioCanvas nodes follow mouse drag", async ({
    page,
  }) => {
    await page.route("**/api/**", async (route) => {
      const url = new URL(route.request().url());
      const p = url.pathname;
      if (p === "/api/auth/me") {
        await route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify({
            user_id: "00000000-0000-0000-0000-000000000001",
            tenant_id: "default",
            username: "ops_admin",
            dept_id: "east",
            roles: ["ADMIN"],
            permissions: ["DATA_READ", "STUDIO_EDIT", "STUDIO_PUBLISH"],
            token_kind: "DEV",
          }),
        });
        return;
      }
      if (p === "/api/xblue/subgraphs") {
        await route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify([
            {
              slug: "main",
              display_name: "企业主图",
              owner_dept: null,
              active_version: 1,
              updated_at: "2026-11-21T00:00:00Z",
            },
          ]),
        });
        return;
      }
      if (p === "/api/xblue/schema") {
        await route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify({
            subgraph: "main",
            version: 1,
            node_types: [
              {
                name: "Factory",
                display_name: "工厂",
                icon: "factory",
                color: "#2563eb",
                title_template: "{node_key}",
                description: "",
                sync_to_knowledge: false,
                pos_x: 100,
                pos_y: 140,
                properties: [],
                derived_metrics: [],
                aggregation_rules: [],
              },
              {
                name: "Warehouse",
                display_name: "仓库",
                icon: "warehouse",
                color: "#059669",
                title_template: "{node_key}",
                description: "",
                sync_to_knowledge: false,
                pos_x: 440,
                pos_y: 140,
                properties: [],
                derived_metrics: [],
                aggregation_rules: [],
              },
            ],
            edge_types: [],
          }),
        });
        return;
      }
      if (p === "/api/xblue/drafts") {
        await route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify([]),
        });
        return;
      }
      if (p === "/api/xblue/schema/validate") {
        await route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify({ valid: true, issues: [] }),
        });
        return;
      }
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ items: [], nodes: [], links: [] }),
      });
    });

    await page.goto("/studio");

    // Verify unified account menu & logout action in the sticky top-right header
    const userMenuBtn = page.getByTestId("shell-user-menu-btn");
    await expect(userMenuBtn).toBeVisible();
    const menuBox = await userMenuBtn.boundingBox();
    expect(menuBox).not.toBeNull();
    expect(menuBox!.y).toBeLessThan(60);
    expect(menuBox!.x).toBeGreaterThan(800);
    await userMenuBtn.click();
    const logoutBtn = page.getByTestId("shell-logout-btn");
    await expect(logoutBtn).toBeVisible();
    await page.keyboard.press("Escape");

    // Verify StudioCanvas node follows mouse during drag (even BEFORE mouseup!)
    const factoryCard = page.getByTestId("canvas-node-Factory");
    await expect(factoryCard).toBeVisible();
    const beforeBox = await factoryCard.boundingBox();
    expect(beforeBox).not.toBeNull();

    const startX = beforeBox!.x + beforeBox!.width / 2;
    const startY = beforeBox!.y + 20;
    await page.mouse.move(startX, startY);
    await page.mouse.down();
    await page.mouse.move(startX + 120, startY + 80, { steps: 8 });

    // Check mid-drag position BEFORE mouse.up()!
    const midDragBox = await factoryCard.boundingBox();
    expect(midDragBox).not.toBeNull();
    expect(midDragBox!.x).toBeGreaterThan(beforeBox!.x + 30);
    expect(midDragBox!.y).toBeGreaterThan(beforeBox!.y + 20);

    await page.mouse.up();
  });
});
