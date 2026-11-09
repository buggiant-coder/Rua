import { expect, test } from "@playwright/test";
import { mkdtempSync, mkdirSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

test.skip(
  process.env.RUA_PROJECT_RESOURCES_E2E !== "1",
  "Requires an isolated local Rua runtime",
);

test("multi-folder project, sidebar organization and project actions against the real runtime", async ({
  page,
  request,
}) => {
  await page.addInitScript(() => localStorage.setItem("language", "en"));
  const name = `Resource acceptance ${Date.now()}`;
  const sectionName = `Acceptance work ${Date.now()}`;
  const api =
    process.env.RUA_PROJECT_RESOURCES_API ?? "http://127.0.0.1:18765/api";
  const devices = await (await request.get(`${api}/sync/devices`)).json();
  const self = devices.devices.find(
    (device: { is_self: boolean }) => device.is_self,
  );
  expect(self).toBeTruthy();
  const root = mkdtempSync(join(tmpdir(), "rua-resources-e2e-"));
  const app = join(root, "app"),
    docs = join(root, "docs");
  mkdirSync(app);
  mkdirSync(docs);
  writeFileSync(join(docs, "guide.txt"), "Resource proof");
  const created = await request.post(`${api}/projects`, {
    data: {
      resource_device_id: self.device_id,
      resource_roots: [app, docs],
      display_name: name,
    },
  });
  expect(created.status()).toBe(201);
  const { project } = await created.json();
  const id = project.project_id;
  const peerName = `${name} peer`;
  const peerResponse = await request.post(`${api}/projects`, {
    data: {
      resource_device_id: self.device_id,
      resource_roots: [app, docs],
      display_name: peerName,
    },
  });
  expect(peerResponse.status()).toBe(201);
  const peer = (await peerResponse.json()).project;
  const peerId = peer.project_id;
  const secondary = project.resources.directories[1].directory_id;
  await page.goto(`/projects/${id}/chats/00000000-0000-4000-8000-000000009901`);
  const path = page.url();
  const actions = () =>
    page.getByRole("button", {
      name: `Actions for ${name}`,
      exact: true,
    });
  await expect(actions()).toBeVisible();
  const headerActions = page.getByRole("button", {
    name: "Project actions",
    exact: true,
  });
  const headerBox = (await headerActions.boundingBox())!;
  const actionsBox = (await actions().boundingBox())!;
  expect(
    Math.abs(headerBox.x + headerBox.width - actionsBox.x - actionsBox.width),
  ).toBeLessThanOrEqual(1);
  const sourceTitle = page.getByRole("button", { name: peerName, exact: true });
  const targetTitle = page.getByRole("button", { name, exact: true });
  await sourceTitle.scrollIntoViewIfNeeded();
  const sourceBox = (await sourceTitle.boundingBox())!;
  const targetBox = (await targetTitle.boundingBox())!;
  const expandedBefore = await sourceTitle.getAttribute("aria-expanded");
  await page.mouse.move(sourceBox.x + 40, sourceBox.y + sourceBox.height / 2);
  await page.mouse.down();
  await page.mouse.move(sourceBox.x + 48, sourceBox.y + sourceBox.height / 2, {
    steps: 3,
  });
  await page.mouse.move(targetBox.x + 40, targetBox.y + targetBox.height - 5, {
    steps: 6,
  });
  await page.mouse.move(targetBox.x + 42, targetBox.y + targetBox.height - 5);
  await expect(page.locator(`[data-project-drop-id="${id}"]`)).toHaveAttribute(
    "data-drop-position",
    "after",
  );
  await expect(page.locator("[data-sidebar-drag-preview]")).toHaveText(
    peerName,
  );
  await page.screenshot({
    path: test.info().outputPath("project-drag-after.png"),
  });
  await page.mouse.move(targetBox.x + 40, targetBox.y + 5, { steps: 4 });
  await expect(page.locator(`[data-project-drop-id="${id}"]`)).toHaveAttribute(
    "data-drop-position",
    "before",
  );
  await page.screenshot({
    path: test.info().outputPath("project-drag-before.png"),
  });
  await page.mouse.up();
  await expect(sourceTitle).toHaveAttribute("aria-expanded", expandedBefore!);
  await expect(page.locator("[data-sidebar-drag-preview]")).toHaveCount(0);

  await expect
    .poll(async () => {
      const state = await (await request.get(`${api}/project-sidebar`)).json();
      return (
        state.sort === "manual" &&
        state.items[peerId]?.order < state.items[id]?.order
      );
    })
    .toBe(true);
  expect(page.url()).toBe(path);
  await actions().click();
  await page
    .getByRole("menuitem", { name: "Edit project", exact: true })
    .click();
  await page
    .getByRole("radio", { name: `Use ${docs} as default folder`, exact: true })
    .check();
  await page.getByRole("button", { name: "Save", exact: true }).click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  expect(
    (await (await request.get(`${api}/projects/${id}`)).json()).resources
      .primary_directory_id,
  ).toBe(secondary);
  await actions().click();
  await page
    .getByRole("menuitem", { name: "Pin project", exact: true })
    .click();
  await expect
    .poll(
      async () =>
        (await (await request.get(`${api}/project-sidebar`)).json()).items[id]
          ?.pinned,
    )
    .toBe(true);
  expect(page.url()).toBe(path);
  await actions().click();
  await page.getByRole("menuitem", { name: "Browse resource folders" }).click();
  await expect(page.getByText("guide.txt", { exact: true })).toBeVisible();
  const workspace = page.getByRole("dialog");
  const workspaceNode = (await workspace.elementHandle())!;
  await expect(page.locator(".rua-ui-drawer-mask")).toHaveClass(/--visible/);
  await workspace.getByRole("tab", { name: "Git", exact: true }).click();
  await workspace.getByRole("combobox").click();
  await page.getByRole("option", { name: app, exact: true }).click();
  expect(await workspaceNode.evaluate((node) => node.isConnected)).toBe(true);
  await expect(page.locator(".rua-ui-drawer-mask")).toHaveClass(/--visible/);
  await expect(
    workspace.getByRole("tab", { name: "Git", exact: true }),
  ).toHaveAttribute("aria-selected", "true");
  await workspace.getByRole("combobox").click();
  await page.getByRole("option", { name: docs, exact: true }).click();
  await workspace.getByRole("tab", { name: "Files", exact: true }).click();
  await expect(page.getByText("guide.txt", { exact: true })).toBeVisible();
  expect(await workspaceNode.evaluate((node) => node.isConnected)).toBe(true);
  await page.getByRole("button", { name: "Close", exact: true }).click();
  await page
    .getByRole("button", { name: "Project actions", exact: true })
    .click();
  await page
    .getByRole("menuitem", { name: "New section", exact: true })
    .click();
  await page.getByRole("textbox", { name: "Section name" }).fill(sectionName);
  await page
    .getByRole("dialog")
    .getByRole("button", { name: /OK|确定/ })
    .click();
  await actions().click();
  await page
    .getByRole("menuitem", { name: "Move to section", exact: true })
    .click();
  await page.getByRole("dialog").getByRole("combobox").click();
  await page.getByRole("option", { name: sectionName, exact: true }).click();
  await page
    .getByRole("dialog")
    .getByRole("button", { name: /OK|确定/ })
    .click();
  await expect(
    page
      .getByRole("region", { name: sectionName })
      .getByRole("button", { name, exact: true }),
  ).toBeVisible();
  expect(page.url()).toBe(path);
  await page
    .getByRole("button", { name: peerName, exact: true })
    .dragTo(page.getByRole("button", { name, exact: true }), {
      targetPosition: { x: 30, y: 6 },
    });
  await expect
    .poll(async () => {
      const state = await (await request.get(`${api}/project-sidebar`)).json();
      return (
        Boolean(state.items[id]?.section_id) &&
        state.items[peerId]?.section_id === state.items[id]?.section_id &&
        state.items[peerId]?.order < state.items[id]?.order
      );
    })
    .toBe(true);
  await actions().click();
  await page.getByRole("menuitem", { name: "Hide from sidebar" }).click();
  await expect(actions()).toHaveCount(0);
  expect(page.url()).toBe(path);
  await page.goto("/projects?overview=1");
  await expect(
    page.getByRole("heading", { name: "Hidden from sidebar" }),
  ).toBeVisible();
  await page.getByRole("button", { name: "Show in sidebar" }).click();
  await expect(actions()).toBeVisible();
  await actions().click();
  await page.getByRole("menuitem", { name: "Archive idle chats" }).click();
  await page
    .getByRole("dialog")
    .getByRole("button", { name: "Archive idle chats" })
    .click();
  await expect(
    page.getByText("Archived 0 · skipped 0 · failed 0"),
  ).toBeVisible();
  await page
    .getByRole("dialog")
    .getByRole("button", { name: "Close", exact: true })
    .last()
    .click();
  await actions().click();
  await expect(page.getByRole("menu")).toHaveCSS("opacity", "1");
  await page.screenshot({
    path: test.info().outputPath("project-menu-desktop.png"),
    fullPage: true,
  });
  await page.keyboard.press("Escape");
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto("/projects?overview=1&_mobile=1");
  const menu = page.getByRole("button", { name: "menu", exact: true });
  await expect(menu).toBeVisible();
  await menu.click();
  await actions().click();
  await expect(page.getByRole("menu")).toHaveCSS("opacity", "1");
  const box = await page.getByRole("menu").boundingBox();
  expect(box).not.toBeNull();
  expect(box!.x).toBeGreaterThanOrEqual(0);
  expect(box!.x + box!.width).toBeLessThanOrEqual(391);
  await page.screenshot({
    path: test.info().outputPath("project-menu-mobile.png"),
    fullPage: true,
  });
  for (const projectId of [id, peerId]) {
    expect(
      (
        await request.post(`${api}/projects/${projectId}/archive`, {
          data: { confirmed: true },
        })
      ).ok(),
    ).toBe(true);
    expect(
      (
        await request.post(`${api}/projects/${projectId}/permanent-delete`, {
          data: { confirmed: true },
        })
      ).ok(),
    ).toBe(true);
  }
  const state = await (await request.get(`${api}/project-sidebar`)).json();
  const section = Object.values(state.sections).find(
    (value) => (value as { name: string }).name === sectionName,
  ) as { section_id: string };
  expect(
    (
      await request.patch(`${api}/project-sidebar`, {
        data: {
          expected_revision: state.revision,
          action: "delete_section",
          section_id: section.section_id,
        },
      })
    ).ok(),
  ).toBe(true);
});

for (const width of [1280, 390]) {
  test(`project folder edit persists after save and reload at ${width}px`, async ({
    page,
    request,
  }) => {
    await page.setViewportSize({ width, height: 900 });
    await page.addInitScript(() => localStorage.setItem("language", "zh"));
    const api =
      process.env.RUA_PROJECT_RESOURCES_API ?? "http://127.0.0.1:18765/api";
    const devices = await (await request.get(`${api}/sync/devices`)).json();
    const device = devices.devices.find(
      (item: { is_self: boolean }) => item.is_self,
    );
    const root = mkdtempSync(join(tmpdir(), "rua-folder-edit-"));
    const app = join(root, "app"),
      docs = join(root, "docs");
    mkdirSync(app);
    mkdirSync(docs);
    const name = `编辑验收 ${Date.now()}`;
    const response = await request.post(`${api}/projects`, {
      data: {
        resource_device_id: device.device_id,
        resource_roots: [app],
        display_name: name,
      },
    });
    expect(response.status()).toBe(201);
    const id = (await response.json()).project.project_id;
    const actions = () =>
      page.getByRole("button", { name: `${name} 的操作`, exact: true });
    const openEdit = async () => {
      await page
        .getByRole("button", { name: "选择项目", exact: true })
        .waitFor();
      if (width < 600 && !(await page.getByRole("navigation").isVisible()))
        await page.getByRole("button", { name: "menu", exact: true }).click();
      await actions().click();
      await page
        .getByRole("menuitem", { name: "编辑项目", exact: true })
        .click();
    };
    try {
      await page.goto(
        `/projects/${id}/chats/00000000-0000-4000-8000-000000009905`,
      );
      await openEdit();
      await expect(page.getByRole("radio")).toHaveCount(1);
      await page
        .getByRole("button", { name: "添加文件夹", exact: true })
        .click();
      await expect(
        page.getByRole("button", { name: "保存", exact: true }),
      ).toHaveCount(0);
      await page.getByRole("button", { name: "docs", exact: true }).click();
      await expect(
        page.getByRole("textbox", { name: "文件夹路径", exact: true }),
      ).toHaveValue(docs);
      await page
        .getByRole("button", { name: "添加文件夹", exact: true })
        .click();
      await expect(page.getByRole("radio")).toHaveCount(2);
      await page
        .getByRole("radio", { name: `将 ${docs} 设为默认目录`, exact: true })
        .check();
      await page.getByRole("button", { name: "保存", exact: true }).click();
      await expect(page.getByRole("dialog")).toHaveCount(0);
      await expect
        .poll(async () => {
          const value = await (
            await request.get(`${api}/projects/${id}`)
          ).json();
          return value.resources.directories.map(
            (item: { display_path: string }) => item.display_path,
          );
        })
        .toEqual([app, docs]);
      await page.reload();
      await openEdit();
      await expect(page.getByRole("radio")).toHaveCount(2);
      await expect(
        page.getByRole("radio", {
          name: `将 ${docs} 设为默认目录`,
          exact: true,
        }),
      ).toBeChecked();
      const dialog = page.getByRole("dialog");
      await expect(dialog).not.toContainText("common.remove");
      await expect(dialog).not.toContainText("用于新建对话");
      await expect(dialog).not.toContainText("移除仅解除绑定");
      await expect(page.locator(".rua-ui-modal-mask")).toHaveCSS(
        "opacity",
        "1",
      );
      const bounds = (await dialog.boundingBox())!;
      expect(bounds.x).toBeGreaterThanOrEqual(0);
      expect(bounds.x + bounds.width).toBeLessThanOrEqual(width);
      await dialog.screenshot({
        path: test.info().outputPath(`project-edit-${width}.png`),
      });
      await page
        .getByRole("button", { name: "添加文件夹", exact: true })
        .click();
      await expect(
        page.getByRole("textbox", { name: "文件夹路径", exact: true }),
      ).toHaveValue(root);
      await expect(dialog).not.toContainText("Storage locations");
      await dialog.screenshot({
        path: test.info().outputPath(`project-folder-picker-${width}.png`),
      });
      await page.getByRole("button", { name: "取消", exact: true }).click();
      await expect(page.getByRole("radio")).toHaveCount(2);
      await page.getByRole("button", { name: "取消", exact: true }).click();
    } finally {
      await request.post(`${api}/projects/${id}/archive`, {
        data: { confirmed: true },
      });
      await request.post(`${api}/projects/${id}/permanent-delete`, {
        data: { confirmed: true },
      });
    }
  });
}
