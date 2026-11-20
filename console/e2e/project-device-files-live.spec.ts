import { expect, test, type Page } from "@playwright/test";
import { readFileSync, writeFileSync } from "node:fs";
import { randomUUID } from "node:crypto";

async function openProjects(page: Page) {
  const setupFile = process.env.RUA_PROJECT_FILE_LIVE;
  test.skip(
    !setupFile,
    "Requires the dedicated Android emulator and Linux desktop runtime",
  );
  const setup: {
    desktopToken?: string;
    desktopId: string;
    phoneId: string;
    root: string;
    output: string;
  } = JSON.parse(readFileSync(setupFile!, "utf8"));
  await page.addInitScript((token?: string) => {
    localStorage.setItem("language", "en");
    if (token) localStorage.setItem("rua_auth_token", token);
  }, setup.desktopToken);
  await page.goto("/projects?overview=1");
  const dialog = page.getByRole("dialog");
  const selectDevice = async (label: string) => {
    await dialog.getByRole("combobox").click();
    await page.getByRole("option").filter({ hasText: label }).click();
  };
  const catalog = await page.evaluate(async () => {
    const token = localStorage.getItem("rua_auth_token"),
      res = await fetch("/api/sync/devices", {
        headers: token ? { Authorization: "Bearer " + token } : {},
      });
    return res.json();
  });
  const deviceLabel = (id: string) => {
    const dev = catalog.devices.find(
      (d: { device_id: string }) => d.device_id === id,
    );
    expect(dev).toBeTruthy();
    return dev.friendly_name || dev.label || id;
  };
  const desktop = deviceLabel(setup.desktopId),
    phone = deviceLabel(setup.phoneId);
  return { setup, desktop, phone, selectDevice };
}

test("a paired Android phone and desktop use their own same-path Git repositories", async ({
  page,
}, testInfo) => {
  test.setTimeout(150000);
  const { setup, phone, desktop, selectDevice } = await openProjects(page);
  const name = "Resource verification " + randomUUID().slice(0, 8);
  const created = page.waitForResponse(
    (r) =>
      new URL(r.url()).pathname === "/api/projects" &&
      r.request().method() === "POST",
  );
  await page
    .getByRole("button", { name: "New Project", exact: true })
    .last()
    .click();
  const dialog = page.getByRole("dialog");
  await dialog.getByRole("textbox", { name: "Display name" }).fill(name);
  await dialog
    .getByRole("button", { name: "Create Project", exact: true })
    .click();
  const creation = await created;
  expect(creation.ok(), await creation.text()).toBe(true);
  const { project } = await creation.json();
  const base = "/api/projects/" + encodeURIComponent(project.project_id);
  const actions = page
    .locator("#desktop-navigation")
    .getByRole("button", { name: "Actions for " + name, exact: true });
  const menu = async (item: string) => {
    await actions.click();
    await page.getByRole("menuitem", { name: item, exact: true }).click();
  };
  await menu("Edit project");
  for (const label of [desktop, phone]) {
    await dialog
      .getByRole("button", { name: "Add folder", exact: true })
      .click();
    await selectDevice(label);
    await dialog.getByRole("textbox", { name: "Folder path" }).fill(setup.root);
    await dialog
      .getByRole("button", { name: "Add folder", exact: true })
      .click();
    await expect(
      dialog.getByRole("button", { name: "Save", exact: true }),
    ).toBeVisible({ timeout: 30000 });
  }
  const resources = page.waitForResponse(
    (r) =>
      new URL(r.url()).pathname === base + "/resources" &&
      r.request().method() === "PATCH",
  );
  await dialog.getByRole("button", { name: "Save", exact: true }).click();
  const saved = await resources;
  expect(saved.ok(), await saved.text()).toBe(true);
  const dirs = (await saved.json()).project.resources.directories;
  expect(
    dirs.map((d: { device_id: string; resource_root: string }) => [
      d.device_id,
      d.resource_root,
    ]),
  ).toEqual([
    [setup.desktopId, setup.root],
    [setup.phoneId, setup.root],
  ]);
  await menu("Edit project");
  await dialog
    .getByRole("button", { name: "Change folder", exact: true })
    .last()
    .click();
  const child = "browser-phone-" + randomUUID().slice(0, 8);
  await dialog.getByRole("button", { name: "New folder", exact: true }).click();
  await dialog.getByRole("textbox", { name: "Folder name" }).fill(child);
  const mkdir = page.waitForResponse(
    (r) => new URL(r.url()).pathname === "/api/projects/directory/create",
  );
  await dialog.getByRole("button", { name: "New folder", exact: true }).click();
  const receipt = await mkdir;
  expect(receipt.ok(), await receipt.text()).toBe(true);
  expect(receipt.request().postDataJSON()).toEqual({
    device_id: setup.phoneId,
    parent: setup.root,
    name: child,
  });
  expect((await receipt.json()).path).toBe(setup.root + "/" + child);
  writeFileSync(
    setup.output,
    JSON.stringify({
      project_id: project.project_id,
      root: setup.root,
      child,
    }) + "\n",
    { mode: 0o600 },
  );
  await dialog
    .getByRole("button", { name: "Cancel", exact: true })
    .last()
    .click();
  await expect(
    dialog.getByRole("button", { name: "Save", exact: true }),
  ).toBeVisible();
  await dialog.getByRole("button", { name: "Cancel", exact: true }).click();
  await menu("Browse resource folders");
  await expect(
    dialog.getByText("only-desktop.txt", { exact: true }),
  ).toBeVisible();
  const phoneListing = () =>
    page.waitForResponse((response) => {
      const url = new URL(response.url());
      return (
        url.pathname === base + "/workspace/files" &&
        url.searchParams.get("device_id") === setup.phoneId &&
        url.searchParams.get("path") === setup.root
      );
    });
  const firstPhoneListing = phoneListing();
  await selectDevice(phone);
  let listing = await firstPhoneListing;
  if (!listing.ok()) {
    // Resource edits replicate independently. A user can refresh the selected
    // folder when the paired phone has not received the new Project yet.
    const pending =
      /Project unavailable on resource Device|Selected location is not registered/;
    expect((await listing.json()).detail).toMatch(pending);
    await expect
      .poll(
        async () => {
          const refreshed = phoneListing();
          await dialog.getByRole("button", { name: "Refresh files" }).click();
          listing = await refreshed;
          if (!listing.ok())
            expect((await listing.json()).detail).toMatch(pending);
          return listing.ok();
        },
        { timeout: 30000, intervals: [1000, 2000, 5000] },
      )
      .toBe(true);
  }
  await expect(
    dialog.getByText("only-phone.txt", { exact: true }),
  ).toBeVisible();
  await expect(dialog.getByText(child, { exact: true })).toBeVisible();
  await expect(
    dialog.getByText("only-desktop.txt", { exact: true }),
  ).toHaveCount(0);
  await dialog.getByRole("button", { name: "Refresh files" }).click();
  await dialog.getByRole("tab", { name: "Git", exact: true }).click();
  await expect(dialog.getByText("phone 32", { exact: true })).toBeVisible({
    timeout: 30000,
  });
  await expect(
    dialog
      .getByRole("tabpanel")
      .getByText("untracked-phone.txt", { exact: true }),
  ).toBeVisible();
  await dialog.getByRole("button", { name: "Load more", exact: true }).click();
  await expect(dialog.getByText("phone 01", { exact: true })).toBeVisible({
    timeout: 30000,
  });
  await testInfo.attach("phone-git-history", {
    body: await dialog.screenshot(),
    contentType: "image/png",
  });
  await selectDevice(desktop);
  await expect(dialog.getByText("desktop 32", { exact: true })).toBeVisible({
    timeout: 30000,
  });
  await expect(dialog.getByText("phone 32", { exact: true })).toHaveCount(0);
});

test("an offline phone keeps its selected resource and the desktop repository stays usable", async ({
  page,
}) => {
  test.skip(
    !process.env.RUA_PROJECT_OFFLINE_LIVE,
    "Requires the paired handset to be stopped",
  );
  test.setTimeout(150000);
  const { setup, phone, desktop, selectDevice } = await openProjects(page);
  const name = process.env.RUA_PROJECT_OFFLINE_LIVE!;
  await page
    .locator("#desktop-navigation")
    .getByRole("button", { name: "Actions for " + name, exact: true })
    .click();
  await page
    .getByRole("menuitem", { name: "Browse resource folders", exact: true })
    .click();
  const dialog = page.getByRole("dialog");
  await expect(
    dialog.getByText("only-desktop.txt", { exact: true }),
  ).toBeVisible();
  const files = (deviceId: string) =>
    page.waitForResponse(
      (response) => {
        const url = new URL(response.url());
        return (
          url.pathname.endsWith("/workspace/files") &&
          url.searchParams.get("device_id") === deviceId &&
          url.searchParams.get("path") === setup.root
        );
      },
      { timeout: 90000 },
    );
  const failed = files(setup.phoneId);
  await selectDevice(phone);
  const failure = await failed;
  expect(failure.status(), await failure.text()).toBe(503);
  const { detail } = await failure.json();
  await expect(dialog.getByText(detail, { exact: true })).toBeVisible();
  await expect(dialog.getByRole("combobox")).toContainText(phone);
  await expect(
    dialog.getByText("only-desktop.txt", { exact: true }),
  ).toHaveCount(0);
  const local = files(setup.desktopId);
  await selectDevice(desktop);
  expect((await local).ok()).toBe(true);
  await expect(
    dialog.getByText("only-desktop.txt", { exact: true }),
  ).toBeVisible();
  await dialog.getByRole("tab", { name: "Git", exact: true }).click();
  await expect(dialog.getByText("desktop 32", { exact: true })).toBeVisible();
});
