import { expect, test, type Page } from "@playwright/test";

async function expectSingleRow(page: Page) {
  const header = page.locator(".rua-owned-chat-header");
  await expect(header).toBeVisible();
  await expect(
    page.getByRole("button", { name: "menu", exact: true }),
  ).toBeVisible();
  await expect
    .poll(() =>
      header.evaluate((element) => {
        const bounds = element.getBoundingClientRect();
        const controls = [...element.querySelectorAll("button")].map((button) =>
          button.getBoundingClientRect(),
        );
        const center = controls[0].y + controls[0].height / 2;
        const menu = document
          .querySelector('button[aria-label="menu"]')!
          .getBoundingClientRect();
        return (
          Math.abs(menu.left - bounds.left) < 1 &&
          menu.width === 36 &&
          menu.height === 40 &&
          Math.abs(controls[0].left - menu.right) < 1 &&
          getComputedStyle(element.querySelector("button")!).paddingLeft ===
            "0px" &&
          Math.abs(menu.y + menu.height / 2 - center) < 1 &&
          controls.every(
            (rect) =>
              Math.abs(rect.y + rect.height / 2 - center) < 1 &&
              rect.x >= bounds.x &&
              rect.right <= bounds.right + 1,
          ) &&
          element.scrollWidth <= element.clientWidth
        );
      }),
    )
    .toBe(true);
}

test.beforeEach(async ({ page }) => {
  await page.route(/\/api\//, (route) =>
    !new URL(route.request().url()).pathname.startsWith("/api/")
      ? route.continue()
      : route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify({
            enabled: false,
            items: [],
            projects: [],
            chats: [],
          }),
        }),
  );
});

for (const mode of ["mobile", "shell"]) {
  test(`${mode} navigation and Agent selector remain independently clickable`, async ({
    page,
  }) => {
    await page.setViewportSize({ width: 360, height: 800 });
    await page.goto(`/e2e/fixtures/chat-header.html?mode=${mode}`);
    await expectSingleRow(page);
    const menu = page.getByRole("button", { name: "menu", exact: true });
    const spacing = await page.evaluate(() => {
      const menu = document.querySelector('button[aria-label="menu"]')!;
      const agent = document.querySelector(
        ".rua-owned-chat-header-scope button",
      )!;
      const inkBounds = (svg: SVGSVGElement) => {
        const box = svg.getBBox();
        const matrix = svg.getScreenCTM()!;
        const stroke = parseFloat(getComputedStyle(svg).strokeWidth) / 2;
        return {
          left: new DOMPoint(box.x - stroke, 0).matrixTransform(matrix).x,
          right: new DOMPoint(box.x + box.width + stroke, 0).matrixTransform(
            matrix,
          ).x,
        };
      };
      const menuInk = inkBounds(menu.querySelector("svg")!);
      const agentInk = inkBounds(agent.querySelector("svg")!);
      return {
        left: menuInk.left - menu.getBoundingClientRect().left,
        right: agentInk.left - menuInk.right,
      };
    });
    expect(Math.abs(spacing.left - spacing.right)).toBeLessThan(1);
    await menu.click();
    if (mode === "mobile") {
      await expect(page.locator(".rua-nav-drawer")).toBeVisible();
      await page.keyboard.press("Escape");
      await expect(page.locator(".rua-nav-drawer")).not.toBeVisible();
    } else {
      await expect(menu).toHaveAttribute("aria-expanded", "true");
      await expect(page.locator("#desktop-navigation")).toBeVisible();
      await menu.click();
      await expect(menu).toHaveAttribute("aria-expanded", "false");
    }
    await expectSingleRow(page);
    const agent = page.getByRole("button", { name: "切换智能体", exact: true });
    await agent.click({ position: { x: 2, y: 16 } });
    await expect(
      page.locator(".rua-ui-popover").getByRole("button", { name: /小Rua/ }),
    ).toBeVisible();
  });
}

for (const mode of ["web", "mobile", "shell"]) {
  for (const width of [320, 360, 390, 430]) {
    for (const project of [false, true]) {
      test(`${mode} ${width}px ${
        project ? "project" : "independent"
      } stays single row with long names`, async ({ page }) => {
        await page.setViewportSize({ width, height: 800 });
        await page.goto(
          `/e2e/fixtures/chat-header.html?mode=${mode}&long&lang=en${
            project ? "&project" : ""
          }`,
        );
        await expectSingleRow(page);
        await expect(
          page.getByRole("button", { name: "New chat", exact: true }),
        ).toBeVisible();
        await page.getByRole("button", { name: "More", exact: true }).click();
        await expect(
          page.getByRole("menuitem", { name: "Tasks", exact: true }),
        ).toBeVisible();
        await page
          .getByRole("menuitem", { name: "Tasks", exact: true })
          .click();
        await expect(page.locator("output")).toHaveText("tasks");
      });
    }
  }
}

test("short labels keep actions visible when they fit, then progressively overflow and restore", async ({
  page,
}) => {
  await page.setViewportSize({ width: 430, height: 800 });
  await page.goto("/e2e/fixtures/chat-header.html?mode=mobile");
  const actions = page.locator(".rua-owned-chat-header-actions button");
  await expect(actions).toHaveCount(4);
  await page.setViewportSize({ width: 360, height: 800 });
  await expectSingleRow(page);
  await expect(actions).toHaveCount(3);
  await page.getByRole("button", { name: "More", exact: true }).click();
  await expect(
    page.getByRole("menuitem", { name: "Tasks", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("menuitem", { name: "Manage chats", exact: true }),
  ).toHaveCount(0);
  await page.getByRole("menuitem", { name: "Existing action" }).click();
  await expect(page.locator("output")).toHaveText("existing");
  await page.setViewportSize({ width: 320, height: 800 });
  await expect(actions).toHaveCount(2);
  await expectSingleRow(page);
  await page.setViewportSize({ width: 430, height: 800 });
  await expect(actions).toHaveCount(4);
  await expectSingleRow(page);
});

test("disabled overflow actions stay disabled and desktop restores all buttons", async ({
  page,
}) => {
  await page.setViewportSize({ width: 320, height: 800 });
  await page.goto(
    "/e2e/fixtures/chat-header.html?mode=mobile&long&disabled&project",
  );
  await expectSingleRow(page);
  await page.getByRole("button", { name: "More", exact: true }).click();
  await expect(
    page.getByRole("menuitem", { name: "Tasks", exact: true }),
  ).toBeDisabled();
  await page
    .getByRole("menuitem", { name: "Manage chats", exact: true })
    .click();
  await expect(page.locator("output")).toHaveText("chats");
  await page.setViewportSize({ width: 1200, height: 800 });
  await expect(
    page.locator(".rua-owned-chat-header-actions button"),
  ).toHaveCount(5);
  await expectSingleRow(page);
});
