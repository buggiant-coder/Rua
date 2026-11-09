import { expect, test } from "@playwright/test";

for (const width of [390, 1440]) {
  test(`ordinary chat list shares row layout at ${width}px`, async ({
    page,
  }) => {
    await page.setViewportSize({ width, height: 844 });
    await page.goto(
      `/e2e/fixtures/collaboration-drawer.html?agent${
        width === 390 ? "&mobile" : ""
      }`,
    );
    const panel = page.getByRole(width === 390 ? "dialog" : "complementary");
    await expect(panel).toBeVisible();
    const row = panel.locator(".rua-owned-chat-list-item--standard");
    const title = await row
      .locator(".rua-owned-chat-list-item-main")
      .boundingBox();
    const actions = await row
      .locator(".rua-owned-chat-list-actions")
      .boundingBox();
    expect(title!.y + title!.height).toBeLessThanOrEqual(actions!.y + 1);
    await expect(row.locator(".rua-owned-chat-pin-slot")).toHaveCount(0);
    if (width === 1440) {
      const article = await page.getByTestId("conversation").boundingBox();
      const bounds = await panel.boundingBox();
      expect(article!.x + article!.width).toBeLessThanOrEqual(bounds!.x);
      const handle = page.getByRole("separator");
      await handle.press("ArrowLeft");
      await expect(handle).toHaveAttribute("aria-valuenow", "400");
      expect((await panel.boundingBox())!.width).toBe(400);
      await page.getByRole("button", { name: "Toggle list" }).click();
      await expect(panel).toHaveCount(0);
    }
  });
}

test("project sidebar animates both directions and supports rapid reversal", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1440, height: 844 });
  await page.goto("/e2e/fixtures/collaboration-drawer.html?project");
  const toggle = page.getByRole("button", { name: "Toggle list" });
  const article = page.getByTestId("conversation");
  const panel = page.locator(".rua-owned-chat-side-panel");
  await expect
    .poll(async () => (await article.boundingBox())!.width)
    .toBe(1060);
  // Sample the active CSS transition deterministically rather than racing wall-clock frames.
  const sample = async (closing: boolean) =>
    page.locator("main").evaluate((stage, closing) => {
      const animation = stage
        .getAnimations()
        .find(
          (item) =>
            item instanceof CSSTransition &&
            item.transitionProperty === "grid-template-columns",
        );
      if (!animation) throw new Error("Missing grid width transition");
      animation.pause();
      animation.currentTime = 90;
      const width = stage
        .querySelector("article")!
        .getBoundingClientRect().width;
      animation.finish();
      return { width, closing };
    }, closing);
  await toggle.click();
  const close = await sample(true);
  expect(close.width).toBeGreaterThan(1060);
  expect(close.width).toBeLessThan(1440);
  await expect(panel).toBeHidden();
  await toggle.click();
  const open = await sample(false);
  expect(open.width).toBeGreaterThan(1060);
  expect(open.width).toBeLessThan(1440);
  await expect(panel).toBeVisible();
  await toggle.dblclick({ delay: 20 });
  await expect
    .poll(async () => (await article.boundingBox())!.width)
    .toBe(1060);
  await page.emulateMedia({ reducedMotion: "reduce" });
  await toggle.click();
  await expect
    .poll(async () => (await article.boundingBox())!.width)
    .toBe(1440);
});

test("project list can resize with pointer and keyboard and remembers width", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1440, height: 844 });
  await page.goto("/e2e/fixtures/collaboration-drawer.html?project");
  const handle = page.getByRole("separator", { name: "调整对话列表宽度" });
  const panel = page.getByRole("complementary");
  await expect(handle).toHaveAttribute("aria-valuenow", "380");
  await expect(handle).toHaveCSS("background-color", "rgba(0, 0, 0, 0)");
  await handle.hover();
  await expect(handle).not.toHaveCSS("background-color", "rgba(0, 0, 0, 0)");
  await page.mouse.move(20, 20);
  await expect(handle).toHaveCSS("background-color", "rgba(0, 0, 0, 0)");
  const bounds = (await handle.boundingBox())!;
  await page.mouse.move(bounds.x + 4, bounds.y + 150);
  await page.mouse.down();
  await expect(handle).toHaveAttribute("data-dragging", "true");
  await page.mouse.move(bounds.x - 116, bounds.y + 150, { steps: 8 });
  await page.mouse.up();
  await expect(handle).toHaveAttribute("aria-valuenow", "500");
  expect((await panel.boundingBox())!.width).toBe(500);
  await page.reload();
  await expect(handle).toHaveAttribute("aria-valuenow", "500");
  await handle.focus();
  await page.keyboard.press("ArrowRight");
  await expect(handle).toHaveAttribute("aria-valuenow", "480");
  await page.keyboard.press("Home");
  await expect(handle).toHaveAttribute("aria-valuenow", "280");
  await page.keyboard.press("End");
  await expect(handle).toHaveAttribute("aria-valuenow", "720");
  await page.setViewportSize({ width: 900, height: 844 });
  await expect(handle).toHaveAttribute("aria-valuenow", "540");
  await handle.dblclick();
  await expect(handle).toHaveAttribute("aria-valuenow", "380");
  await page.goto("/e2e/fixtures/collaboration-drawer.html?project&mobile");
  await expect(page.getByRole("separator")).toHaveCount(0);
});

for (const width of [390, 1440]) {
  test(`project list uses ${
    width === 390 ? "overlay" : "reserved layout"
  } at ${width}px`, async ({ page }, testInfo) => {
    await page.setViewportSize({ width, height: 844 });
    await page.goto(
      `/e2e/fixtures/collaboration-drawer.html?project${
        width === 390 ? "&mobile" : ""
      }`,
    );
    const panel =
      width === 390
        ? page.getByRole("dialog")
        : page.getByRole("complementary");
    await expect(panel).toBeVisible();
    await expect
      .poll(async () => {
        const bounds = await panel.boundingBox();
        return bounds ? bounds.x + bounds.width : Infinity;
      })
      .toBeLessThanOrEqual(width + 1);
    if (width === 1440) {
      const article = await page.getByTestId("conversation").boundingBox();
      const bounds = await panel.boundingBox();
      expect(article!.x + article!.width).toBeLessThanOrEqual(bounds!.x);
      await expect(page.getByRole("dialog")).toHaveCount(0);
      await expect(panel.locator(":scope > header")).toHaveCount(0);
    }
    await expect(
      panel.getByRole("button", { name: "新建项目对话" }),
    ).toHaveCount(0);
    const row = panel.locator(".rua-owned-chat-list-item--standard");
    await expect(
      row.locator(".rua-owned-chat-pin-slot, .rua-owned-chat-channel-text"),
    ).toHaveCount(0);
    const title = await row
      .locator(".rua-owned-chat-list-item-main")
      .boundingBox();
    const actions = await row
      .locator(".rua-owned-chat-list-actions")
      .boundingBox();
    expect(title!.y + title!.height).toBeLessThanOrEqual(actions!.y + 1);
    await page.screenshot({
      path: testInfo.outputPath("project-list.png"),
      animations: "disabled",
    });
    await panel.getByRole("button", { name: "归档对话", exact: true }).click();
    await expect(row).toHaveCount(0);
    await panel.getByRole("tab", { name: "归档", exact: true }).click();
    await expect(panel.getByRole("tab")).toHaveText([
      "我的对话",
      "协作对话",
      "归档",
    ]);
    await expect(
      panel.getByRole("button", { name: "归档", exact: true }),
    ).toHaveCount(0);
    await expect(
      panel.getByRole("tab", { name: "归档", exact: true }),
    ).toHaveAttribute("aria-selected", "true");
    await panel.getByRole("tab", { name: "协作对话", exact: true }).click();
    await expect(
      panel.getByRole("tab", { name: "协作对话", exact: true }),
    ).toHaveAttribute("aria-selected", "true");
    await panel.getByRole("tab", { name: "归档", exact: true }).click();
    await panel.getByRole("button", { name: "恢复对话", exact: true }).click();
    await expect(row).toHaveCount(0);
    await panel.getByRole("tab", { name: "我的对话", exact: true }).click();
    await expect(row).toHaveCount(1);
  });
}

for (const width of [390, 1440]) {
  test(`collaboration drawer is compact and localized at ${width}px`, async ({
    page,
  }, testInfo) => {
    await page.setViewportSize({ width, height: 844 });
    await page.goto(
      `/e2e/fixtures/collaboration-drawer.html${
        width === 390 ? "?mobile" : ""
      }`,
    );
    await expect(page.getByRole("tab", { name: "协作对话" })).toHaveAttribute(
      "aria-selected",
      "true",
    );
    await expect(
      page.getByRole("button", { name: "新建项目对话" }),
    ).toHaveCount(0);
    const row = page.locator(".rua-owned-collaboration-row");
    await expect(row).toContainText("ACP · codex");
    await expect(row).toContainText("运行中");
    await expect(row).toContainText("来源可用");
    await expect(
      row.locator(
        ".rua-owned-chat-channel-tag, .rua-owned-chat-tag-action, .rua-owned-chat-pin-slot",
      ),
    ).toHaveCount(0);
    await expect(row.locator(".rua-owned-chat-list-actions")).toHaveCSS(
      "grid-column-start",
      "2",
    );
    await expect(row.locator(".rua-owned-chat-list-actions")).toHaveCSS(
      "grid-row-start",
      "1",
    );
    await expect(page.getByPlaceholder("来源对话 ID")).toHaveCount(0);
    await expect
      .poll(async () => {
        const bounds = await row.boundingBox();
        return bounds ? bounds.x + bounds.width : Infinity;
      })
      .toBeLessThanOrEqual(width);
    await page.screenshot({ path: testInfo.outputPath("collaboration.png") });
    await page.getByRole("button", { name: "筛选与排序" }).click();
    await expect(page.getByPlaceholder("来源对话 ID")).toBeVisible();
    await expect(
      page.getByRole("combobox", { name: "按状态筛选" }),
    ).toBeVisible();
    await expect(
      page.locator(".rua-owned-chat-tag-filter-options"),
    ).toHaveCount(0);
    await page.keyboard.press("Escape");
    await page.getByRole("tab", { name: "我的对话" }).click();
    await expect(
      page.getByRole("button", { name: "新建项目对话" }),
    ).toBeVisible();
  });
}
