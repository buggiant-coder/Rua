import { expect, test } from "@playwright/test";
import { mockConsoleTransport } from "./fixtures/consoleTransport";
type Resource = {
  directory_id?: string;
  device_id: string;
  resource_root: string;
  display_name?: string;
};
type Project = {
  project_id: string;
  display_name: string;
  resources: { revision: number; directories: Resource[] };
  lifecycle: string;
};
test("a Project can be empty and keep same-path folders on two devices independently", async ({
  page,
}) => {
  await mockConsoleTransport(page);
  await page.addInitScript(() => {
    localStorage.setItem("language", "en");
    localStorage.setItem("rua_sider_collapsed", "false");
  });
  let project: Project | undefined;
  let phoneOnline = true;
  const browses: string[] = [];
  const writes: unknown[] = [];
  const devices = () => [
    {
      device_id: "workstation",
      friendly_name: "Workstation",
      label: "Workstation",
      is_self: true,
      connectivity_state: "connected",
    },
    {
      device_id: "handset",
      friendly_name: "Handset",
      label: "Handset",
      is_self: false,
      connectivity_state: phoneOnline ? "connected" : "offline",
    },
  ];
  await page.route(
    (url) => url.pathname.startsWith("/api/"),
    async (route) => {
      const request = route.request(),
        path = new URL(request.url()).pathname.slice(4),
        method = request.method();
      const json = (body: unknown, status = 200) =>
        route.fulfill({ json: body, status });
      if (path === "/auth/status") return json({ enabled: false });
      if (path === "/agents")
        return json({
          agents: [{ id: "default", name: "Default", enabled: true }],
        });
      if (path === "/models/active")
        return json({ default_llm: { provider_id: "fixture", model: "m" } });
      if (path === "/sync/devices") return json({ devices: devices() });
      if (path === "/project-sidebar")
        return json({ revision: "1", sort: "manual", items: {} });
      if (path === "/projects" && method === "GET")
        return json({
          projects:
            project && !request.url().includes("lifecycle=archived")
              ? [project]
              : [],
        });
      if (path === "/projects" && method === "POST") {
        const body = request.postDataJSON();
        writes.push(body);
        project = {
          project_id: "photos",
          display_name: body.display_name,
          resources: { revision: 1, directories: body.directories },
          lifecycle: "active",
        };
        return json({ project, overlap_warnings: [] }, 201);
      }
      if (path === "/projects/photos" && method === "GET") return json(project);
      if (path === "/projects/directory") {
        const { device_id, path: input } = request.postDataJSON();
        browses.push(device_id + ":" + input);
        if (device_id === "handset" && !phoneOnline)
          return json({ detail: "Handset offline" }, 503);
        if (input === "/missing")
          return json({ detail: "No such directory on " + device_id }, 404);
        const root = input === "~" ? "/" : input;
        return json({
          device_id,
          path: root,
          parent_path: root === "/" ? null : "/",
          roots: [],
          entries:
            root === "/"
              ? [{ name: "shared", path: "/shared", kind: "directory" }]
              : [],
        });
      }
      if (
        path === "/projects/photos/resources" &&
        method === "PATCH" &&
        project
      ) {
        const body = request.postDataJSON();
        writes.push(body);
        if (body.expected_revision !== project.resources.revision)
          return json({ detail: "stale revision" }, 409);
        if (
          body.directories.some(
            (d: Resource) =>
              !d.directory_id && d.device_id === "handset" && !phoneOnline,
          )
        )
          return json({ detail: "Handset offline" }, 503);
        project = {
          ...project,
          resources: {
            revision: project.resources.revision + 1,
            directories: body.directories.map((d: Resource, i: number) => ({
              ...d,
              directory_id: d.directory_id ?? "location-" + i,
            })),
          },
        };
        return json({ project, overlap_warnings: [] });
      }
      if (path === "/chats" || path.endsWith("/chats"))
        return route.fulfill({ json: [], headers: { "X-Total-Count": "0" } });
      if (
        path.includes("/providers") ||
        path.includes("/channels") ||
        path === "/models"
      )
        return json([]);
      return json({});
    },
  );
  await page.goto("/projects?overview=1");
  await page
    .getByRole("button", { name: "New Project", exact: true })
    .last()
    .click();
  const dialog = page.getByRole("dialog");
  await dialog
    .getByRole("textbox", { name: "Display name" })
    .fill("Photo collection");
  await dialog
    .getByRole("button", { name: "Create Project", exact: true })
    .click();
  await expect.poll(() => project?.resources.directories).toEqual([]);
  expect(writes[0]).toEqual({
    display_name: "Photo collection",
    directories: [],
    execution_default: null,
  });
  expect(browses).toEqual([]);
  const sidebar = page.locator("#desktop-navigation");
  const edit = async () => {
    await sidebar
      .getByRole("button", {
        name: "Actions for Photo collection",
        exact: true,
      })
      .click();
    await page
      .getByRole("menuitem", { name: "Edit project", exact: true })
      .click();
    await expect(
      dialog.getByRole("button", { name: "Save", exact: true }),
    ).toBeVisible();
  };
  const add = async (device: string) => {
    await dialog
      .getByRole("button", { name: "Add folder", exact: true })
      .click();
    if (device === "Handset") {
      await dialog.getByRole("combobox").click();
      await page.getByRole("option", { name: "Handset", exact: true }).click();
    }
    await dialog.getByRole("button", { name: "shared", exact: true }).click();
    await expect(
      dialog.getByRole("textbox", { name: "Folder path" }),
    ).toHaveValue("/shared");
    await dialog
      .getByRole("button", { name: "Add folder", exact: true })
      .click();
    await expect(
      dialog.getByRole("button", { name: "Save", exact: true }),
    ).toBeVisible();
  };
  await edit();
  await add("Workstation");
  await add("Handset");
  await expect(dialog.getByText("/shared", { exact: true })).toHaveCount(2);
  phoneOnline = false;
  await dialog.getByRole("button", { name: "Save", exact: true }).click();
  await expect(dialog.getByRole("alert")).toContainText("Handset offline");
  expect(project!.resources.directories).toHaveLength(0);
  phoneOnline = true;
  await dialog.getByRole("button", { name: "Save", exact: true }).click();
  await expect(dialog).toHaveCount(0);
  expect(project!.resources.directories.map((d) => d.device_id)).toEqual([
    "workstation",
    "handset",
  ]);
  const phoneId = project!.resources.directories[1].directory_id;
  phoneOnline = false;
  await page.reload();
  await edit();
  await expect(dialog.getByText("/shared", { exact: true })).toHaveCount(2);
  const visits = browses.length;
  await dialog
    .getByRole("textbox", { name: "Resource name" })
    .last()
    .fill("Phone photos");
  await dialog.getByRole("button", { name: "Detach folder" }).first().click();
  await dialog.getByRole("button", { name: "Save", exact: true }).click();
  await expect(dialog).toHaveCount(0);
  expect(project!.resources.directories).toEqual([
    {
      directory_id: phoneId,
      device_id: "handset",
      resource_root: "/shared",
      display_name: "Phone photos",
    },
  ]);
  await edit();
  await dialog.getByRole("button", { name: "Detach folder" }).click();
  await dialog.getByRole("button", { name: "Save", exact: true }).click();
  await expect(dialog).toHaveCount(0);
  await page.reload();
  await edit();
  await expect(dialog.getByText("No resource folders")).toBeVisible();
  expect(project!.project_id).toBe("photos");
  expect(browses.length).toBe(visits);
});
