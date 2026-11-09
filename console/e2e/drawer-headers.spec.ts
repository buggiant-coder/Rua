import { expect, test } from "@playwright/test";

for (const mode of ["desktop", "mobile", "standalone"]) {
  for (const width of mode === "desktop" ? [1440, 700] : [390]) {
    for (const chat of [false, true]) {
      test(`${mode} ${width} ${
        chat ? "Chat" : "PageHeader"
      }: portal header aligns`, async ({ page }) => {
        await page.setViewportSize({ width, height: 900 });
        await page.goto(
          `/e2e/fixtures/drawer-headers.html?mode=${mode}${
            chat ? "&chat" : ""
          }`,
        );
        const header = page.locator(".rua-ui-drawer-header");
        const reference = page.locator(
          chat ? '[data-testid="page-header"]' : ".reference-header",
        );
        await expect(header).toHaveCSS(
          "height",
          mode === "standalone" ? "56px" : "48px",
        );
        await expect
          .poll(async () => {
            const actual = await header.boundingBox();
            const expected = await reference.boundingBox();
            return actual && expected
              ? Math.abs(
                  actual.y + actual.height - expected.y - expected.height,
                )
              : Number.POSITIVE_INFINITY;
          })
          .toBeLessThan(0.5);
        const title = page.locator(".rua-ui-drawer-title");
        await expect(title).toHaveCSS("text-overflow", "ellipsis");
        await expect(
          page.getByRole("button", { name: "Close", exact: true }),
        ).toBeInViewport();
        await page.locator(".rua-ui-drawer-body").evaluate((body) => {
          body.scrollTop = 600;
        });
        expect((await header.boundingBox())?.y).toBe(0);
      });
    }
  }
}

for (const kind of ["editor", "library"]) {
  test(`mobile ${kind}: custom title row aligns above action toolbar`, async ({
    page,
  }) => {
    await page.setViewportSize({ width: 390, height: 844 });
    await page.goto(
      `/e2e/fixtures/drawer-headers.html?mode=mobile&kind=${kind}`,
    );
    const toolbar = page.locator(
      '[class*="editorHeaderDrawer"] [class*="buttonGroup"]',
    );
    await expect(toolbar).toBeVisible();
    expect((await toolbar.boundingBox())?.y).toBe(48);
    await expect(page.locator(".rua-ui-drawer-body")).toHaveCSS(
      "padding",
      "0px",
    );
    await expect(
      page.locator('[class*="editorHeaderDrawer"] [class*="fileName"]'),
    ).toHaveCSS("text-overflow", "ellipsis");
    for (const button of await toolbar.getByRole("button").all()) {
      await expect(button).toBeInViewport();
    }
  });
}

test("mobile navigation drawer uses the same header height", async ({
  page,
}) => {
  await page.goto(
    "/e2e/fixtures/drawer-headers.html?mode=mobile&kind=navigation&placement=left&dark",
  );
  await expect(page.getByTestId("navigation-header")).toHaveCSS(
    "height",
    "48px",
  );
});
