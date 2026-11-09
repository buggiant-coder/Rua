import { expect, test } from "@playwright/test";

for (const width of [1440, 390]) {
  test(`phone card matches desktop cards at ${width}px`, async ({
    page,
  }, testInfo) => {
    await page.setViewportSize({ width, height: 1000 });
    await page.route(/\/api\//, async (route) => {
      const path = new URL(route.request().url()).pathname;
      let body: unknown = {};
      let status = 200;
      if (path === "/api/auth/status")
        body = { enabled: false, has_users: false };
      if (path === "/api/agents") body = { agents: [] };
      if (path === "/api/projects") body = { projects: [] };
      if (path === "/api/models" || path === "/api/models/providers") body = [];
      if (path === "/api/sync/status") body = { enabled: false };
      if (path === "/api/language") body = { language: "zh" };
      if (path === "/api/version") body = { version: "0.1.0" };
      if (path === "/api/platform/browser")
        body = {
          available: true,
          state: "ready",
          selected_path: null,
          resolved_path: "/Applications/Chrome",
          diagnostics: [],
        };
      if (path === "/api/platform/computer")
        body = {
          available: true,
          state: "ready",
          platform: "macos",
          permissions: {
            accessibility: true,
            screen_recording: true,
            input: true,
          },
          diagnostics: [],
        };
      if (path === "/api/platform/phone") {
        status = 503;
        body = { detail: "Phone Use requires an Android execution device" };
      }
      await route.fulfill({
        status,
        contentType: "application/json",
        body: JSON.stringify(body),
      });
    });
    await page.goto("/desktop-tools");
    if (width < 640)
      await page.getByRole("button", { name: "menu", exact: true }).click();
    const phone = page.getByRole("region", { name: "手机操作", exact: true });
    await expect(phone.getByText("当前执行设备不支持手机操作")).toBeVisible();
    await expect(page.getByText(/Phone Use requires/)).toHaveCount(0);
    const styleOf = (element: Element) => {
      const style = getComputedStyle(element);
      const title = getComputedStyle(element.querySelector("h2")!);
      return {
        padding: style.padding,
        border: style.border,
        radius: style.borderRadius,
        titleSize: title.fontSize,
      };
    };
    const phoneStyle = await phone.evaluate(styleOf);
    expect(Number.parseFloat(phoneStyle.padding)).toBeGreaterThanOrEqual(16);
    for (const name of ["浏览器操作", "电脑操作"]) {
      expect(
        await page.getByRole("region", { name, exact: true }).evaluate(styleOf),
      ).toEqual(phoneStyle);
    }
    await expect(phone.getByRole("button", { name: "重新检测" })).toBeVisible();
    expect(
      await phone.evaluate((el) => el.scrollWidth <= el.clientWidth + 1),
    ).toBe(true);
    expect(
      await page.evaluate(() => document.documentElement.scrollWidth),
    ).toBeLessThanOrEqual(width);
    await page.screenshot({
      path: testInfo.outputPath(`phone-card-${width}.png`),
      fullPage: true,
    });
  });
}
