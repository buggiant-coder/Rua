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

test("resource browsing, Git, and folder creation follow the selected Device and actual path", async ({
  page,
}) => {
  await mockConsoleTransport(page);
  await page.addInitScript(() => localStorage.setItem("language", "en"));
  const root = "/shared/repo";
  const devices = ["desktop", "phone"].map((device_id) => ({
    device_id,
    friendly_name: device_id,
    is_self: device_id === "desktop",
    connectivity_state: "connected",
  }));
  const project = {
    project_id: "pair",
    display_name: "Pair",
    lifecycle: "active",
    resources: {
      revision: 1,
      directories: devices.map((d) => ({
        directory_id: "row-" + d.device_id,
        device_id: d.device_id,
        resource_root: root,
      })),
    },
  };
  const workspace: string[] = [];
  const mkdir: unknown[] = [];
  const unconfirmed =
    "Directory creation on Device 'phone' is unconfirmed. Refresh the directory before trying again.";
  let online = true;
  const listing = (
    device: string,
    path: string,
    name: string,
    offset: number | null = null,
  ) => ({
    device_id: device,
    path,
    parent_path: "/",
    roots: [],
    proposed_name: "repo",
    entries: [
      {
        name,
        path: path + "/" + name,
        kind: name === "nested" ? "directory" : "file",
        is_symlink: false,
      },
    ],
    next_offset: offset,
  });
  await page.route(
    (u) => u.pathname.startsWith("/api/"),
    (route) => {
      const req = route.request(),
        url = new URL(req.url()),
        path = url.pathname.slice(4),
        query = url.searchParams;
      const send = (json: unknown, status = 200) =>
        route.fulfill({ json, status });
      if (path === "/agents")
        return send({
          agents: [{ id: "default", name: "Default", enabled: true }],
        });
      if (path === "/auth/status") return send({ enabled: false });
      if (path === "/sync/devices") return send({ devices });
      if (path === "/project-sidebar")
        return send({ revision: "1", sections: [], items: {} });
      if (path === "/projects") return send({ projects: [project] });
      if (path === "/projects/pair") return send(project);
      if (path === "/projects/directory") {
        const { device_id, path: real } = req.postDataJSON();
        return send(listing(device_id, real, "nested"));
      }
      if (path === "/projects/directory/create") {
        const body = req.postDataJSON();
        mkdir.push(body);
        if (body.name === "receipt-lost")
          return send({ detail: unconfirmed }, 503);
        return send(
          listing(body.device_id, body.parent + "/" + body.name, "nested"),
        );
      }
      if (path.startsWith("/projects/pair/workspace/")) {
        const device = query.get("device_id"),
          base = query.get("resource_root"),
          offset = Number(query.get("offset") ?? 0),
          operation = path.split("workspace/")[1],
          actual = query.get("path");
        workspace.push([device, operation, actual ?? base, offset].join(" "));
        if (
          !devices.some((d) => d.device_id === device) ||
          query.has("directory_id") ||
          base !== root
        )
          return send({ detail: "Wrong resource target" }, 400);
        if (device === "phone" && !online)
          return send({ detail: "Phone offline" }, 503);
        if (operation === "files" && actual === root)
          return send(
            listing(
              device!,
              root,
              offset ? device + ".txt" : "nested",
              offset ? null : 1,
            ),
          );
        if (operation === "files" && actual === root + "/nested")
          return send(listing(device!, actual, "child-" + device + ".txt"));
        if (operation === "git")
          return send({
            repository: true,
            branch: device + "-branch",
            detached: false,
            ahead: 0,
            behind: 0,
            changes: [],
          });
        if (operation === "git/history")
          return send({
            items: [
              {
                oid: (offset ? "b" : "a").repeat(40),
                parent_oids: [],
                summary: device + " commit " + (offset + 1),
                authored_at: "2026-11-20T00:00:00Z",
                refs: [],
              },
            ],
            offset,
            limit: 30,
            total: 2,
          });
        return send({ detail: "Wrong operation or path" }, 400);
      }
      if (
        path === "/chats" ||
        path === "/models" ||
        path.includes("/providers") ||
        path.includes("/channels")
      )
        return send([]);
      return send({});
    },
  );
  await page.goto("/projects?overview=1");
  const actions = page
    .locator("#desktop-navigation")
    .getByRole("button", { name: "Actions for Pair", exact: true });
  const dialog = page.getByRole("dialog");
  await actions.click();
  await page.getByRole("menuitem", { name: "Browse resource folders" }).click();
  await expect(dialog.getByText("nested", { exact: true })).toBeVisible();
  const select = async (device: string) => {
    await dialog.getByRole("combobox").click();
    await page.getByRole("option").filter({ hasText: device }).click();
  };
  await select("phone");
  await expect(dialog.getByRole("button", { name: "nested" })).toBeVisible();
  const pageError = dialog
    .getByRole("alert")
    .filter({ hasText: "Phone offline" });
  const moreFiles = dialog.getByRole("button", { name: "Load more" });
  const refreshFiles = dialog.getByRole("button", { name: "Refresh files" });
  online = false;
  await refreshFiles.click();
  await expect(pageError).toContainText(root);
  await expect(dialog.getByRole("button", { name: "nested" })).toBeVisible();
  await expect(moreFiles).toHaveCount(0);
  online = true;
  await refreshFiles.click();
  await expect(pageError).toHaveCount(0);
  online = false;
  await moreFiles.click();
  await expect(pageError).toContainText(
    "Could not list all entries in " + root,
  );
  await expect(dialog.getByRole("combobox")).toContainText("phone");
  online = true;
  await dialog.getByText("nested", { exact: true }).click();
  await expect(dialog.getByText("child-phone.txt")).toBeVisible();
  await expect(pageError).toContainText(root);
  await moreFiles.click();
  await expect(dialog.getByText("phone.txt", { exact: true })).toBeVisible();
  await expect(dialog.getByText("Phone offline")).toHaveCount(0);
  await expect(dialog.getByText("child-phone.txt")).toBeVisible();
  expect(
    workspace.filter((v) => v === "phone files /shared/repo 1"),
  ).toHaveLength(2);
  await dialog.getByRole("tab", { name: "Git", exact: true }).click();
  await expect(dialog.getByText("phone commit 1")).toBeVisible();
  const moreCommits = dialog.getByRole("button", { name: "Load more" });
  online = false;
  await moreCommits.click();
  await expect(dialog.getByText(/Phone offline/)).toBeVisible();
  await expect(dialog.getByText("phone commit 1")).toBeVisible();
  await expect(dialog.getByText("phone-branch")).toBeVisible();
  await expect(dialog.getByRole("alert")).toContainText(/commit history/i);
  online = true;
  await moreCommits.click();
  await expect(dialog.getByText("phone commit 2")).toBeVisible();
  await expect(dialog.getByText("phone commit 1")).toHaveCount(1);
  await expect(dialog.getByText(/Phone offline/)).toHaveCount(0);
  expect(
    workspace.filter((v) => v === "phone git/history /shared/repo 1"),
  ).toHaveLength(2);
  expect(workspace.filter((v) => v.startsWith("phone git "))).toHaveLength(1);
  online = false;
  await dialog.getByRole("button", { name: "Refresh Git status" }).click();
  await expect(dialog.getByText("Phone offline")).toBeVisible();
  await expect(dialog.getByRole("combobox")).toContainText("phone");
  await select("desktop");
  await expect(dialog.getByText("desktop commit 1")).toBeVisible();
  expect(workspace).toEqual(
    expect.arrayContaining([
      "phone files /shared/repo 0",
      "phone files /shared/repo/nested 0",
      "phone files /shared/repo 1",
      "phone git/history /shared/repo 1",
    ]),
  );
  await dialog.getByRole("button", { name: "Close", exact: true }).click();
  online = true;
  await actions.click();
  await page.getByRole("menuitem", { name: "Edit project" }).click();
  await dialog.getByRole("button", { name: "Change folder" }).last().click();
  await dialog.getByRole("button", { name: "New folder", exact: true }).click();
  await dialog
    .getByRole("textbox", { name: "Folder name" })
    .fill("phone-write");
  await dialog.getByRole("button", { name: "New folder", exact: true }).click();
  await expect
    .poll(() => mkdir)
    .toEqual([{ device_id: "phone", parent: root, name: "phone-write" }]);
  await dialog.getByRole("button", { name: "New folder", exact: true }).click();
  await dialog
    .getByRole("textbox", { name: "Folder name" })
    .fill("receipt-lost");
  await dialog.getByRole("button", { name: "New folder", exact: true }).click();
  await expect(dialog.getByRole("alert")).toHaveText(unconfirmed);
  await dialog
    .getByRole("button", { name: "Cancel", exact: true })
    .first()
    .click();
  expect(mkdir).toEqual([
    { device_id: "phone", parent: root, name: "phone-write" },
    { device_id: "phone", parent: root + "/phone-write", name: "receipt-lost" },
  ]);
});
