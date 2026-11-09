import { expect, test, type Page } from "@playwright/test";

const integrityError =
  "MCP config invalid: Desktop bundle signature or sealed component integrity is invalid";
const chromePath =
  "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";

async function openSettings(
  page: Page,
  width = 1440,
  ready = false,
  desktopShell = false,
) {
  await page.setViewportSize({ width, height: 1000 });
  if (desktopShell)
    await page.addInitScript(() => {
      Object.defineProperty(window, "__TAURI_INTERNALS__", {
        value: {
          invoke: async (command: string) => {
            throw new Error(
              `Unexpected Tauri command in desktop settings: ${command}`,
            );
          },
        },
      });
    });
  await page.route(/\/api\//, async (route) => {
    if (!new URL(route.request().url()).pathname.startsWith("/api/"))
      return route.continue();
    const path = new URL(route.request().url()).pathname.replace(/^\/api/, "");
    let body: unknown = {};
    if (path === "/auth/status") body = { enabled: false, has_users: false };
    if (path === "/agents") body = { agents: [] };
    if (path === "/language") body = { language: "zh" };
    if (path === "/version") body = { version: "0.1.0" };
    if (
      path === "/platform/desktop/install" ||
      path === "/platform/desktop/cli"
    )
      body = {
        platform: "macos",
        cliPath: "/Users/test/.local/bin/rua",
        cliInstalled: path.endsWith("/cli"),
        cliConflict: false,
        legacyRuntime: false,
        legacyPort: null,
      };
    if (
      path === "/platform/desktop/update" ||
      path === "/platform/desktop/update/check"
    )
      body = {
        phase: path.endsWith("/check") ? "up_to_date" : "idle",
        currentVersion: "0.1.0",
        availableVersion: null,
        body: null,
        downloaded: 0,
        total: null,
        error: null,
      };
    if (path === "/platform/browser" || path === "/platform/browser/recheck")
      body = {
        device_id: "desktop-test",
        available: ready,
        state: ready ? "ready" : "unavailable",
        selected_path: null,
        resolved_path: chromePath,
        driver_version: "0.38.1",
        diagnostics: ready ? [] : [integrityError],
      };
    if (path === "/platform/computer" || path === "/platform/computer/recheck")
      body = {
        device_id: "desktop-test",
        available: ready,
        state: ready ? "permission_required" : "unavailable",
        platform: "macos",
        driver_version: "0.28.2",
        permissions: ready
          ? {
              accessibility: true,
              screen_recording: false,
              input: true,
              integrity_level: null,
            }
          : null,
        diagnostics: ready
          ? ["Grant Rua Screen Recording permission in system settings."]
          : [integrityError],
      };
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify(body),
    });
  });
  await page.goto("/desktop-tools");
  await expect(
    page.getByRole("heading", { name: "浏览器操作", exact: true }),
  ).toBeVisible();
  await expect(page.getByText(chromePath, { exact: true })).toBeVisible();
}

async function expectNoOverflow(page: Page) {
  const overflowing = await page
    .locator("section")
    .evaluateAll(
      (sections) =>
        sections.filter(
          (section) => section.scrollWidth > section.clientWidth + 1,
        ).length,
    );
  expect(overflowing).toBe(0);
  expect(
    await page.evaluate(() => document.documentElement.scrollWidth),
  ).toBeLessThanOrEqual(await page.evaluate(() => window.innerWidth));
}

test("desktop cards have spacing and explain the actual integrity failure", async ({
  page,
}, testInfo) => {
  await page.addInitScript(() =>
    localStorage.setItem("rua_sider_collapsed", "false"),
  );
  await openSettings(page);
  await expect(
    page.getByRole("button", { name: "menu", exact: true }),
  ).toHaveAttribute("aria-expanded", "true");
  await page.screenshot({
    path: testInfo.outputPath("desktop-settings.png"),
    fullPage: true,
  });
  const browserCard = page.getByRole("region", {
    name: "浏览器操作",
    exact: true,
  });
  const padding = await browserCard.evaluate((element) =>
    Number.parseFloat(getComputedStyle(element).paddingTop),
  );
  expect(padding).toBeGreaterThanOrEqual(20);
  await expect(
    page.getByRole("heading", { name: "桌面工具", level: 1 }),
  ).toBeVisible();
  await expect(page.getByText("安装包异常", { exact: true })).toHaveCount(2);
  await expect(page.getByText(integrityError).first()).not.toBeVisible();
  await expect(
    page.getByRole("button", { name: "为 Rua 授权", exact: true }),
  ).toHaveCount(0);
  await browserCard.getByText("技术详情", { exact: true }).click();
  await expect(browserCard.getByText(integrityError)).toBeVisible();
  await expectNoOverflow(page);
});

for (const width of [720, 390]) {
  test(`settings wrap long paths and actions at ${width}px`, async ({
    page,
  }, testInfo) => {
    await openSettings(page, width);
    await page.getByText("更换浏览器", { exact: true }).click();
    await page
      .getByRole("textbox", { name: "Chromium 程序路径" })
      .fill(chromePath.repeat(3));
    await expectNoOverflow(page);
    await page.screenshot({
      path: testInfo.outputPath(`desktop-settings-${width}.png`),
      fullPage: true,
    });
  });
}

test("partial permissions identify the missing grant instead of hiding all capabilities", async ({
  page,
}, testInfo) => {
  await openSettings(page, 1440, true);
  const computer = page.getByRole("region", { name: "电脑操作", exact: true });
  await expect(computer.getByText("已授权", { exact: true })).toBeVisible();
  await expect(computer.getByText("待授权", { exact: true })).toBeVisible();
  await expect(
    computer.getByText("需要系统授权", { exact: true }),
  ).toBeVisible();
  await expect(page.getByText("安装包异常", { exact: true })).toHaveCount(0);
  await page.screenshot({
    path: testInfo.outputPath("desktop-settings-permissions.png"),
    fullPage: true,
  });
});

test("dark appearance preserves status and permission readability", async ({
  page,
}, testInfo) => {
  await page.addInitScript(() => localStorage.setItem("rua-theme", "dark"));
  await openSettings(page, 1440, true);
  await expect(page.locator("html")).toHaveClass(/dark-mode/);
  await expect(
    page.getByRole("heading", { name: "桌面工具", level: 1 }),
  ).toBeVisible();
  await expectNoOverflow(page);
  await page.screenshot({
    path: testInfo.outputPath("desktop-settings-dark.png"),
    fullPage: true,
  });
});

for (const desktopShell of [false, true]) {
  test(`${
    desktopShell ? "App" : "WebUI"
  } separates desktop tools from About and keeps the menu order`, async ({
    page,
  }, testInfo) => {
    await page.addInitScript(() =>
      localStorage.setItem("rua_sider_collapsed", "false"),
    );
    await openSettings(page, 1440, true, desktopShell);
    const nav = page.getByRole("navigation");
    const labels = await nav.getByRole("button").allTextContents();
    expect(labels[labels.indexOf("工具") + 1]).toBe("桌面工具");
    expect(labels.at(-1)).toBe("关于");
    await expect(
      nav.getByRole("button", { name: "桌面应用", exact: true }),
    ).toHaveCount(0);
    await expect(
      nav.getByRole("button", { name: "桌面工具", exact: true }),
    ).toHaveClass(/navItemActive/);
    await expect(
      page.getByRole("heading", { name: "应用更新", exact: true }),
    ).toHaveCount(0);
    await expect(
      page.getByRole("heading", { name: "命令行工具", exact: true }),
    ).toHaveCount(0);
    const computer = page.getByRole("region", {
      name: "电脑操作",
      exact: true,
    });
    await expect(
      computer.getByRole("button", { name: "为 Rua 授权", exact: true }),
    ).toHaveCount(1);
    const permissionRequest = page.waitForRequest(
      (request) =>
        new URL(request.url()).pathname ===
          "/api/platform/computer/permissions" && request.method() === "POST",
    );
    await computer
      .getByRole("button", { name: "为 Rua 授权", exact: true })
      .click();
    await permissionRequest;
    await expect(
      computer.getByRole("button", { name: "重新检测", exact: true }),
    ).toBeEnabled();
    const recheck = page.waitForRequest(
      (request) =>
        new URL(request.url()).pathname === "/api/platform/computer/recheck" &&
        request.method() === "POST",
    );
    await computer
      .getByRole("button", { name: "重新检测", exact: true })
      .click();
    await recheck;
    await expect(computer.getByRole("alert")).toHaveCount(0);
    await expectNoOverflow(page);
    await page.screenshot({
      path: testInfo.outputPath(
        `desktop-tools-${desktopShell ? "app" : "web"}.png`,
      ),
      fullPage: true,
    });
    await nav.getByRole("button", { name: "关于", exact: true }).click();
    await expect(page).toHaveURL(/\/about$/);
    await expect(
      page.getByRole("heading", { name: "关于", level: 1 }),
    ).toBeVisible();
    await expect(
      page.getByRole("heading", { name: "应用更新", exact: true }),
    ).toBeVisible();
    await expect(
      page.getByRole("heading", { name: "命令行工具", exact: true }),
    ).toBeVisible();
    await expect(
      page.getByRole("heading", { name: "浏览器操作", exact: true }),
    ).toHaveCount(0);
    await expect(
      page.getByRole("button", { name: "安装命令行工具", exact: true }),
    ).toHaveCount(1);
    await expect(
      page.getByText("当前版本：0.1.0", { exact: true }),
    ).toBeVisible();
    const install = page.waitForRequest(
      (request) =>
        new URL(request.url()).pathname === "/api/platform/desktop/cli" &&
        request.method() === "POST",
    );
    await page
      .getByRole("button", { name: "安装命令行工具", exact: true })
      .click();
    await install;
    await expect(
      page.getByRole("button", { name: "已安装", exact: true }),
    ).toBeDisabled();
    const update = page.waitForRequest(
      (request) =>
        new URL(request.url()).pathname ===
          "/api/platform/desktop/update/check" && request.method() === "POST",
    );
    await page.getByRole("button", { name: "检查更新", exact: true }).click();
    await update;
    await expect(page.getByText("已是最新版本", { exact: true })).toBeVisible();
    await expectNoOverflow(page);
    await page.screenshot({
      path: testInfo.outputPath(`about-${desktopShell ? "app" : "web"}.png`),
      fullPage: true,
    });
  });
}

test("old desktop settings links redirect to desktop tools without requiring the app", async ({
  page,
}) => {
  await openSettings(page);
  await page.goto("/settings/desktop");
  await expect(page).toHaveURL(/\/desktop-tools$/);
  await expect(
    page.getByRole("heading", { name: "桌面工具", level: 1 }),
  ).toBeVisible();
});
