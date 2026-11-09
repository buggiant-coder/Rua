import { expect, test, type Page } from "@playwright/test";

async function mockProjectApis(page: Page) {
  await page.route("**/api/**", async (route) => {
    const url = new URL(route.request().url());
    if (!url.pathname.startsWith("/api/")) {
      return route.continue();
    }
    const path = url.pathname.replace(/^\/api/, "");
    const json = (body: unknown, status = 200) =>
      route.fulfill({
        status,
        contentType: "application/json",
        body: JSON.stringify(body),
      });

    if (path === "/auth/status") {
      return json({ enabled: false, has_users: false });
    }
    if (path === "/sync/status") {
      return json({ enabled: false, paired: false, peer_count: 0 });
    }
    if (path === "/sync/devices/status") {
      return json({ revocation_state: "active", is_revoked: false });
    }
    if (path === "/sync/devices") {
      return json({
        devices: [
          {
            device_id: "device-mac",
            public_key: "mac-key",
            friendly_name: "Studio Mac",
            label: "macOS",
            added_at_ms: 1,
            last_seen_ms: Date.now(),
            is_self: true,
            connectivity_state: "connected",
            session_path: null,
            session_rtt_ms: null,
          },
          {
            device_id: "device-phone",
            public_key: "phone-key",
            friendly_name: "Mobile Resource Device With A Long Name",
            label: "Android",
            added_at_ms: 1,
            last_seen_ms: Date.now() - 120_000,
            is_self: false,
            connectivity_state: "not_connected",
            session_path: null,
            session_rtt_ms: null,
          },
        ],
      });
    }
    if (path === "/projects" && route.request().method() === "GET") {
      return json({
        projects: [
          {
            project_id: "project-rua",
            display_name: "rua-rust",
            resource_device_id: "device-mac",
            display_path:
              "/Users/example/MyProjects/rua-rust-with-a-long-worktree-name",
            lifecycle: "active",
            created_at: "2026-08-01T00:00:00Z",
            updated_at: "2026-08-01T00:00:00Z",
            availability: {
              project_id: "project-rua",
              resource_device_id: "device-mac",
              status: "available",
              observed_at: "2026-08-01T00:00:00Z",
              fresh: true,
              detail: "resource Device and directory are available",
            },
          },
          {
            project_id: "project-archive",
            display_name: "Archived mobile app",
            resource_device_id: "device-phone",
            display_path: "/sdcard/Projects/mobile-app",
            lifecycle: "archived",
            created_at: "2026-07-01T00:00:00Z",
            updated_at: "2026-07-01T00:00:00Z",
            availability: {
              project_id: "project-archive",
              resource_device_id: "device-phone",
              status: "device_revoked",
              observed_at: "2026-08-01T00:00:00Z",
              fresh: true,
              detail: "resource Device is not a current network member",
            },
          },
        ],
      });
    }
    if (path === "/projects/directory" && route.request().method() === "POST") {
      return json({
        device_id: "device-mac",
        path: "/Users/example/MyProjects",
        parent_path: "/Users/example",
        proposed_name: "MyProjects",
        entries: [
          {
            name: "rua-rust",
            path: "/Users/example/MyProjects/rua-rust",
            kind: "directory",
            is_symlink: false,
          },
          {
            name: "current",
            path: "/Users/example/MyProjects/current",
            kind: "directory",
            is_symlink: true,
          },
        ],
      });
    }
    return json({});
  });
}

async function expectNoHorizontalScroll(page: Page) {
  const metrics = await page.evaluate(() => ({
    documentWidth: document.documentElement.scrollWidth,
    viewportWidth: document.documentElement.clientWidth,
    bodyWidth: document.body.scrollWidth,
  }));
  expect(metrics.documentWidth).toBeLessThanOrEqual(metrics.viewportWidth + 1);
  expect(metrics.bodyWidth).toBeLessThanOrEqual(metrics.viewportWidth + 1);
}

async function inspectProjects(page: Page, mobile: boolean) {
  await mockProjectApis(page);
  await page.setViewportSize(
    mobile ? { width: 390, height: 844 } : { width: 1440, height: 900 },
  );
  await page.goto(
    mobile ? "/projects?_mobile=1&overview=1" : "/projects?overview=1",
    {
      waitUntil: "networkidle",
    },
  );
  await expect(page.getByText("rua-rust", { exact: true })).toBeVisible();
  await expect(page.getByText("Archived mobile app")).toBeVisible();
  await expect(
    page
      .locator("span:visible")
      .filter({ hasText: /Available|可用/ })
      .first(),
  ).toBeVisible();
  await expect(
    page
      .locator("span:visible")
      .filter({ hasText: /Device removed|设备已移除/ })
      .first(),
  ).toBeVisible();
  await expectNoHorizontalScroll(page);

  await page
    .getByRole("button", { name: /New Project|新建项目/ })
    .first()
    .click();
  await expect(
    page.getByRole("dialog").getByText(/New Project|新建项目/),
  ).toBeVisible();
  await page.getByPlaceholder("~").fill("/Users/example/MyProjects");
  await page.getByRole("button", { name: /Browse directory|浏览目录/ }).click();
  await expect(page.getByText("current")).toBeVisible();
  await expect(
    page.locator('[aria-label="Symbolic link"], [aria-label="符号链接"]'),
  ).toHaveCount(1);
  await expectNoHorizontalScroll(page);

  const dialog = await page.getByRole("dialog").boundingBox();
  expect(dialog).not.toBeNull();
  expect(dialog!.x).toBeGreaterThanOrEqual(0);
  expect(dialog!.x + dialog!.width).toBeLessThanOrEqual(
    (mobile ? 390 : 1440) + 1,
  );
}

test("Projects overview and creation dialog fit desktop", async ({
  page,
}, testInfo) => {
  await inspectProjects(page, false);
  await page.screenshot({
    path: testInfo.outputPath("projects-desktop.png"),
    fullPage: false,
  });
});

test("Projects overview and creation dialog fit mobile", async ({
  page,
}, testInfo) => {
  await inspectProjects(page, true);
  await page.screenshot({
    path: testInfo.outputPath("projects-mobile.png"),
    fullPage: false,
  });
});
