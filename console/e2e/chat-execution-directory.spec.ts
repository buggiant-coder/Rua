import { expect, test, type Page } from "@playwright/test";
import { mockConsoleTransport } from "./fixtures/consoleTransport";
import type {
  ExecutionPosition,
  ResolvedExecutionDirectory,
} from "../src/api/types/chat";

async function setup(page: Page, withPeer = false) {
  await mockConsoleTransport(page);
  await page.addInitScript(() => {
    localStorage.setItem("language", "en");
    localStorage.setItem("rua_sider_collapsed", "false");
  });
  let project: {
    project_id: string;
    display_name: string;
    execution_default: ExecutionPosition | null;
    resources: { revision: number; directories: unknown[] };
    lifecycle: string;
  } | null = null;
  const saved = new Map<
    string,
    { directory: ResolvedExecutionDirectory; projectId?: string }
  >();
  const firstTurns: Array<Record<string, any>> = [];
  const patches: ExecutionPosition[] = [];
  let pendingApproval = false;
  const browsed: Array<[string, string]> = [];
  const directories = new Set([
    "/",
    "/work",
    "/work/first",
    "/work/second",
    "/standalone",
    "/next",
  ]);
  const self = {
    device_id: "service",
    friendly_name: "Service desktop",
    is_self: true,
    connectivity_state: "connected",
  };
  const peer = {
    device_id: "peer",
    friendly_name: "Peer workstation",
    is_self: false,
    connectivity_state: "connected",
  };
  const resolve = (
    id: string,
    pos: ExecutionPosition,
  ): ResolvedExecutionDirectory => ({
    device_id: pos.device_id,
    path: pos.path ?? "/managed/" + id,
    managed: !pos.path,
  });
  await page.context().route(
    (url) => url.pathname.startsWith("/api/"),
    async (route) => {
      const req = route.request(),
        url = new URL(req.url()),
        path = url.pathname.slice(4),
        method = req.method();
      const json = (body: unknown, status = 200) =>
        route.fulfill({ json: body, status });
      if (path === "/auth/status") return json({ enabled: false });
      if (path === "/agents")
        return json({
          agents: [{ id: "default", name: "Default", enabled: true }],
        });
      if (path === "/sync/devices")
        return json({ devices: withPeer ? [self, peer] : [self] });
      if (path === "/sync/status")
        return json({ enabled: withPeer, paired: withPeer });
      if (path === "/models/active")
        return json({ default_llm: { provider_id: "fixture", model: "m" } });
      if (path === "/project-sidebar")
        return json({ revision: "1", sort: "manual", items: {} });
      if (path === "/projects" && method === "GET")
        return json({ projects: project ? [project] : [] });
      if (path === "/projects" && method === "POST") {
        const data = req.postDataJSON();
        project = {
          project_id: "paths",
          display_name: data.display_name,
          execution_default: data.execution_default,
          resources: { revision: 1, directories: data.directories },
          lifecycle: "active",
        };
        return json({ project, overlap_warnings: [] });
      }
      if (path === "/projects/paths" && method === "GET") return json(project);
      if (
        project &&
        path === "/projects/paths/execution-default" &&
        method === "PUT"
      ) {
        project.execution_default = req.postDataJSON().execution_default;
        return json(project);
      }
      if (path === "/chats" || path.endsWith("/chats"))
        return route.fulfill({ json: [], headers: { "X-Total-Count": "0" } });
      const browse = path.match(/^\/devices\/([^/]+)\/execution-directory$/);
      if (browse) {
        const input = url.searchParams.get("path") ?? "",
          root = input === "~" ? "/" : input;
        browsed.push([browse[1], input]);
        if (!directories.has(root))
          return json(
            { detail: "Directory unavailable on " + browse[1] + ": " + input },
            404,
          );
        return json({
          device_id: browse[1],
          path: root,
          parent_path: root === "/" ? null : "/",
          roots: [],
          entries: [...directories]
            .filter(
              (x) =>
                x !== root &&
                x.startsWith(root === "/" ? "/" : root + "/") &&
                !x.slice(root === "/" ? 1 : root.length + 1).includes("/"),
            )
            .map((x) => ({
              name: x.split("/").pop(),
              path: x,
              kind: "directory",
            })),
        });
      }
      const chat = path.match(/^\/chats\/([^/]+)(?:\/(.+))?$/);
      if (chat) {
        const [, id, action] = chat,
          record = saved.get(id);
        if (action === "console/submit") {
          const body = req.postDataJSON();
          firstTurns.push(body);
          saved.set(id, {
            directory: resolve(id, body.execution_directory),
            projectId: body.project_id,
          });
          return json({
            accepted: true,
            mode: "started",
            chat_id: id,
            submission_id: body.submission_id,
          });
        }
        if (!record) return json({ detail: "Chat not found" }, 404);
        if (!action)
          return json({
            id,
            agent_id: "default",
            project_id: record.projectId ?? null,
            title: "Saved context",
            status: "idle",
            meta: { execution_directory: record.directory },
            messages: [{ id: "m", role: "user", content: "Saved context" }],
          });
        if (action === "execution-directory" && method === "PATCH") {
          const pos = req.postDataJSON();
          patches.push(pos);
          if (pendingApproval)
            return json(
              {
                detail:
                  "Execution directory change blocked by unresolved_approval",
              },
              409,
            );
          if (
            pos.device_id !== record.directory.device_id ||
            (pos.path && !directories.has(pos.path))
          )
            return json(
              {
                detail:
                  "Directory unavailable on " + pos.device_id + ": " + pos.path,
              },
              409,
            );
          record.directory = resolve(id, pos);
          return json(record.directory);
        }
        if (action === "agent-selection")
          return json({
            selection: { agent_id: "default", revision: 1 },
            selected_agent_available: true,
            blockers: [],
            switchable: true,
          });
        if (action === "tasks") return json([]);
        if (action === "approvals/snapshot")
          return json({ pending: [], resolved: [] });
        if (action === "acp/permissions") return json({ pending: [] });
      }
      if (
        path.includes("/providers") ||
        path.includes("/channels") ||
        path === "/models"
      )
        return json([]);
      return json({});
    },
  );
  return {
    firstTurns,
    patches,
    browsed,
    getProject: () => project,
    setPendingApproval: (pending: boolean) => {
      pendingApproval = pending;
    },
  };
}
const locationButton = (page: Page) =>
  page
    .locator(".rua-owned-composer")
    .getByRole("button", { name: "Execution location", exact: true });
async function select(page: Page, name: string, option: string) {
  await page
    .getByRole("dialog")
    .getByRole("combobox", { name, exact: true })
    .click();
  await page.getByRole("option", { name: option, exact: true }).click();
}
async function specify(page: Page, path: string) {
  const dialog = page.getByRole("dialog");
  if (
    !(await dialog
      .getByRole("textbox", { name: "Execution directory path" })
      .count())
  )
    await select(page, "Working directory", "Choose a directory");
  await dialog
    .getByRole("textbox", { name: "Execution directory path" })
    .fill(path);
}

test("an unpaired service supplies the standalone default and only its current Device can change a saved Chat directory", async ({
  page,
}) => {
  const state = await setup(page);
  await page.goto("/");
  const location = locationButton(page);
  await expect(location).toBeEnabled();
  await expect(location).toContainText("This device");
  await location.click();
  await specify(page, "/missing");
  await page.getByRole("button", { name: "Save", exact: true }).click();
  await expect(page.getByRole("alert")).toContainText(
    "Directory unavailable on service",
  );
  await specify(page, "/standalone");
  await page.getByRole("button", { name: "Save", exact: true }).click();
  await expect(location).toContainText("/standalone");
  await page.reload();
  await expect(location).toContainText("/standalone");
  await page.locator(".rua-owned-composer textarea").fill("Saved context");
  await page.locator(".rua-owned-composer textarea").press("Enter");
  await expect.poll(() => state.firstTurns.length).toBe(1);
  expect(state.firstTurns[0]).toMatchObject({
    target_device_id: "service",
    execution_directory: { device_id: "service", path: "/standalone" },
  });
  await page.reload();
  await location.click();
  await expect(
    page.getByRole("combobox", { name: "Execution device" }),
  ).toHaveAttribute("aria-disabled", "true");
  await specify(page, "/missing");
  await page.getByRole("button", { name: "Save", exact: true }).click();
  await expect(page.getByRole("alert")).toContainText(
    "Directory unavailable on service",
  );
  await specify(page, "/next");
  state.setPendingApproval(true);
  await page.getByRole("button", { name: "Save", exact: true }).click();
  await expect(page.getByRole("alert")).toContainText("pending approval");
  await expect(location).toContainText("/standalone");
  state.setPendingApproval(false);
  await page.getByRole("button", { name: "Save", exact: true }).click();
  await expect(location).toContainText("/next");
  await location.click();
  await select(page, "Working directory", "Chat-managed directory");
  await page.getByRole("button", { name: "Save", exact: true }).click();
  await expect(location).toContainText("/managed/");
  await page.reload();
  await expect(location).toContainText("/managed/");
  expect(state.patches.at(-1)).toEqual({ device_id: "service" });
});

test("Project execution defaults survive creation and stay independent of a restored draft", async ({
  page,
}) => {
  const state = await setup(page, true);
  await page.goto("/projects?overview=1");
  await page
    .getByRole("button", { name: "New Project", exact: true })
    .last()
    .click();
  const dialog = page.getByRole("dialog");
  await dialog.getByRole("textbox", { name: "Display name" }).fill("Work");
  await select(page, "Execution device", "Peer workstation");
  await select(page, "Working directory", "Choose a directory");
  await dialog.getByRole("button", { name: "Browse directory" }).click();
  await dialog.getByRole("button", { name: "work", exact: true }).click();
  await dialog.getByRole("button", { name: "first", exact: true }).click();
  await dialog
    .getByRole("button", { name: "Create Project", exact: true })
    .click();
  const location = locationButton(page);
  await expect(location).toContainText("/work/first");
  expect(state.getProject()?.resources.directories).toEqual([]);
  expect(state.getProject()?.execution_default).toEqual({
    device_id: "peer",
    path: "/work/first",
  });
  const draftUrl = page.url();
  await page
    .locator("#desktop-navigation")
    .getByRole("button", { name: "Actions for Work" })
    .click();
  await page.getByRole("menuitem", { name: "Edit project" }).click();
  await specify(page, "/work/second");
  await dialog.getByRole("button", { name: "Save", exact: true }).click();
  await expect
    .poll(() => state.getProject()?.execution_default?.path)
    .toBe("/work/second");
  await page.reload();
  await expect(location).toContainText("/work/first");
  const restored = await page.context().newPage();
  await mockConsoleTransport(restored);
  await restored.goto(draftUrl);
  await expect(locationButton(restored)).toContainText("/work/first");
  await restored.close();
  expect(page.url()).toBe(draftUrl);
  await page.locator(".rua-owned-composer textarea").fill("Saved context");
  await page.locator(".rua-owned-composer textarea").press("Enter");
  await expect.poll(() => state.firstTurns.length).toBe(1);
  expect(state.firstTurns[0]).toMatchObject({
    project_id: "paths",
    target_device_id: "peer",
    execution_directory: { device_id: "peer", path: "/work/first" },
  });
  await page.reload();
  await expect(location).toContainText("/work/first");
  await page
    .locator(".rua-owned-chat-header")
    .getByRole("button", { name: "New Project Chat", exact: true })
    .click();
  await expect(location).toContainText("/work/second");
  expect(page.url()).not.toBe(draftUrl);
  await location.click();
  await select(page, "Execution device", "Service desktop");
  await expect(
    dialog.getByRole("textbox", { name: "Execution directory path" }),
  ).toHaveCount(0);
  await dialog.getByRole("button", { name: "Save", exact: true }).click();
  await expect(location).toContainText("Chat-managed directory");
  await page
    .locator("#desktop-navigation")
    .getByRole("button", { name: "Actions for Work" })
    .click();
  await page.getByRole("menuitem", { name: "Edit project" }).click();
  await select(page, "Execution device", "Connected Rua service");
  await dialog.getByRole("button", { name: "Save", exact: true }).click();
  await expect.poll(() => state.getProject()?.execution_default).toBeNull();
  await expect(location).toContainText("Chat-managed directory");
  expect(state.browsed).toContainEqual(["peer", "/work/first"]);
  expect(state.browsed).not.toContainEqual(["service", "/work/first"]);
});

test("the selected directory matches real tool execution and survives a return to the managed directory", async ({
  page,
  request,
}) => {
  const external = process.env.RUA_EXECUTION_DIRECTORY_EXTERNAL;
  test.skip(
    !external,
    "Requires the isolated Rua service and loopback tool provider",
  );
  await page.addInitScript(() => localStorage.setItem("language", "en"));
  await page.goto("/");
  const location = locationButton(page);
  await expect(location).toBeEnabled();
  const id = new URL(page.url()).pathname.split("/").at(-1);
  const head = async () => {
    const result = await request.get("/api/chats/" + id);
    expect(result.ok()).toBe(true);
    return result.json();
  };
  async function turn(key: string, read = false) {
    const input = page.locator(".rua-owned-composer textarea");
    await input.fill(key + (read ? " read the marker" : ""));
    await input.press("Enter");
    await expect(
      page.locator('[data-rua-chat-role="assistant"]').last(),
    ).toContainText("finished " + key, { timeout: 60000 });
    await expect
      .poll(async () => {
        const r = await request.get("/api/chats/" + id + "/agent-selection");
        return r.ok() && (await r.json()).switchable === true;
      })
      .toBe(true);
    return (await head()).meta
      .execution_directory as ResolvedExecutionDirectory;
  }
  await location.click();
  await specify(page, external!);
  await page
    .getByRole("dialog")
    .getByRole("button", { name: "Save", exact: true })
    .click();
  await expect(location).toContainText(external!);
  const explicit = await turn("probe-07-browserfirst", true);
  expect(explicit).toMatchObject({ path: external, managed: false });
  await page.reload();
  await expect(location).toContainText(explicit.path);
  await location.click();
  await select(page, "Working directory", "Chat-managed directory");
  await page
    .getByRole("dialog")
    .getByRole("button", { name: "Save", exact: true })
    .click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  const managed = await turn("probe-07-browserdefault");
  expect(managed.managed).toBe(true);
  expect(managed.path).not.toBe(explicit.path);
  await location.click();
  await specify(page, external! + "/missing");
  await page
    .getByRole("dialog")
    .getByRole("button", { name: "Save", exact: true })
    .click();
  await expect(page.getByRole("alert")).toBeVisible();
  expect((await head()).meta.execution_directory).toEqual(managed);
  await specify(page, external!);
  await page
    .getByRole("dialog")
    .getByRole("button", { name: "Save", exact: true })
    .click();
  await expect(location).toContainText(explicit.path);
  expect(await turn("probe-07-browserexternal", true)).toEqual(explicit);
  await location.click();
  await select(page, "Working directory", "Chat-managed directory");
  await page
    .getByRole("dialog")
    .getByRole("button", { name: "Save", exact: true })
    .click();
  await expect(location).toContainText(managed.path);
  await page.reload();
  await expect(location).toContainText(managed.path);
  expect((await head()).meta.execution_directory).toEqual(managed);
});
