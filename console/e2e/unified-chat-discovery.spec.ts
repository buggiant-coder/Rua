import { expect, test, type Page } from "@playwright/test";
import { mockConsoleTransport } from "./fixtures/consoleTransport";

const agentNames = { default: "Default", research: "Research" };
const projectNames = { photos: "Photos", albums: "Albums" };
const model = { provider_id: "fixture", model: "photo-model" };

function chat(
  number: number,
  name: string,
  projectId: keyof typeof projectNames | null,
  agentId: keyof typeof agentNames,
) {
  return {
    id: `00000000-0000-4000-8000-${String(number).padStart(12, "0")}`,
    name,
    project_id: projectId,
    agent_id: agentId,
    user_id: "fixture",
    channel: "console",
    status: "idle",
    pinned: false,
    archived: false,
    collaboration: false,
    created_at: "2026-01-01T00:00:00Z",
    updated_at: "2026-01-01T00:00:00Z",
  };
}

const photo = {
  ...chat(1, "Pinned photo catalog", "photos", "research"),
  pinned: true,
};
const standalone = chat(2, "Standalone lens notes", null, "research");
const otherAgent = chat(3, "Default photo notes", "photos", "default");
const album = chat(4, "Album proofs", "albums", "research");
const archived = {
  ...chat(5, "Retired photo notes", "photos", "research"),
  archived: true,
};
const collaboration = {
  ...chat(6, "Photo collaboration handoff", "photos", "research"),
  collaboration: true,
};
const pages = Array.from({ length: 72 }, (_, i) =>
  chat(10 + i, `Page item ${i + 1}`, null, "default"),
);
const chats = [
  photo,
  standalone,
  otherAgent,
  album,
  archived,
  collaboration,
  ...pages,
];

async function mockDiscoveryServer(
  page: Page,
  selection = { rejectOnce: "", blockers: [] as string[] },
) {
  const rowsById = chats.map((row) => ({ ...row }));
  const revisions = new Map<string, number>();
  const projects = Object.entries(projectNames).map(
    ([project_id, display_name]) => ({
      project_id,
      display_name,
      display_path: `/fixture/${project_id}`,
      resource_device_id: "desktop",
      lifecycle: "active",
    }),
  );
  const queries: URL[] = [];
  await mockConsoleTransport(page);
  await page.addInitScript(() => {
    localStorage.setItem("language", "en");
    localStorage.setItem("rua_sider_collapsed", "false");
  });
  await page.route(
    (url) => url.pathname.startsWith("/api/"),
    async (route) => {
      const url = new URL(route.request().url());
      const path = url.pathname.slice(4);
      const json = (value: unknown) => route.fulfill({ json: value });
      if (path === "/auth/status") return json({ enabled: false });
      if (path === "/agents")
        return json({
          agents: Object.entries(agentNames).map(([id, name]) => ({
            id,
            name,
            enabled: true,
          })),
        });
      if (path === "/projects") return json({ projects });
      if (path === "/project-sidebar")
        return json({ revision: "1", sort: "manual", items: {} });
      const project = projects.find(
        (item) => path === `/projects/${item.project_id}`,
      );
      if (project) return json(project);
      if (path === "/models/active") return json({ default_llm: model });
      if (path === "/sync/devices") return json({ devices: [] });
      const scopePath = /^\/projects\/([^/]+)\/chats(?:\/search)?$/.exec(path);
      if (path === "/chats" || path === "/chats/search" || scopePath) {
        queries.push(url);
        const q = url.searchParams;
        const projectId = scopePath?.[1] || q.get("project_id");
        const rows = rowsById.filter(
          (row) =>
            (!projectId || row.project_id === projectId) &&
            (q.get("without_project") !== "true" || !row.project_id) &&
            (!q.has("agent_id") || row.agent_id === q.get("agent_id")) &&
            (q.get("view") !== "user" || !row.collaboration) &&
            (!q.has("archived") ||
              row.archived === (q.get("archived") === "true")),
        );
        const offset = Number(q.get("offset") || 0);
        const limit = Number(q.get("limit") || 50);
        if (path.endsWith("/search")) {
          const matches = rows.filter((row) =>
            row.name.toLowerCase().includes(q.get("q")!.toLowerCase()),
          );
          const results = matches.slice(offset, offset + limit).map((row) => ({
            chat_id: row.id,
            chat_name: row.name,
            card_index: 0,
            role: "assistant",
            snippet: `Search hit in ${row.name}`,
            message_id: `message-${row.id}`,
            turn_id: `turn-${row.id}`,
          }));
          return json({
            results,
            offset,
            limit,
            total: matches.length,
            next_offset:
              offset + results.length < matches.length
                ? offset + results.length
                : null,
          });
        }
        return route.fulfill({
          json: rows.slice(offset, offset + limit),
          headers: { "X-Total-Count": String(rows.length) },
        });
      }
      const selecting = rowsById.find(
        (row) => path === `/chats/${row.id}/agent-selection`,
      );
      if (selecting) {
        const blockers = selecting.collaboration
          ? ["fixed_collaboration_executor"]
          : selection.blockers;
        if (route.request().method() === "PATCH") {
          const { expected_agent_id, agent_id } = route
            .request()
            .postDataJSON();
          if (
            selection.rejectOnce ||
            blockers.length ||
            expected_agent_id !== selecting.agent_id
          ) {
            const detail =
              selection.rejectOnce || blockers.join(", ") || "Agent changed";
            selection.rejectOnce = "";
            return route.fulfill({ status: 409, json: { detail } });
          }
          selecting.agent_id = agent_id;
          revisions.set(selecting.id, (revisions.get(selecting.id) ?? 1) + 1);
        }
        return json({
          selection: {
            agent_id: selecting.agent_id,
            revision: revisions.get(selecting.id) ?? 1,
            selected_at: selecting.updated_at,
          },
          selected_agent_available: true,
          switchable: !blockers.length,
          blockers,
        });
      }
      const hit = /^\/chats\/([^/]+)(?:\/metadata)?$/.exec(path);
      const row = hit && rowsById.find((item) => item.id === hit[1]);
      if (row) {
        if (path.endsWith("/metadata")) return json(row);
        return json({
          ...row,
          title: row.name,
          meta: row.collaboration
            ? {
                collaboration: {
                  source: { chat_id: photo.id },
                  source_available: true,
                  executor: { kind: "agent", agent_id: row.agent_id },
                  executor_agent_id: row.agent_id,
                  configuration: {
                    status: "ready",
                    desired_source_epoch: "1",
                    applied_source_epoch: "1",
                  },
                },
              }
            : {},
          model,
          messages: [
            {
              id: `message-${row.id}`,
              turn_id: `turn-${row.id}`,
              role: "assistant",
              metadata: {
                metadata: {
                  turn_agent_assignment: {
                    agent_id: "previous",
                    agent_name: "Original Turn Agent",
                  },
                },
              },
              content: [
                { type: "text", text: `Saved history for ${row.name}` },
              ],
            },
          ],
        });
      }
      if (path.endsWith("/tasks")) return json({ tasks: [] });
      if (
        path === "/models" ||
        path.includes("/templates") ||
        path.includes("/channels") ||
        path.includes("/providers")
      )
        return json([]);
      return json({});
    },
  );
  return queries;
}

test("one discovery panel lists and searches across Projects with the same visibility and Chat identity", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  const queries = await mockDiscoveryServer(page);
  await page.goto("/");
  const sidebar = page.locator("#desktop-navigation");
  const search = sidebar.getByRole("button", {
    name: "Search chats",
    exact: true,
  });
  const panel = sidebar.getByRole("region", { name: "Chat discovery" });
  await search.click();
  const lookup = panel.getByRole("searchbox", { name: "Search chats" });
  const project = panel.getByRole("combobox", { name: "Project filter" });
  const agent = panel.getByRole("combobox", { name: "Agent filter" });
  const archiveFilter = panel.getByRole("combobox", { name: "Archive filter" });
  for (const row of [photo, standalone, otherAgent, album])
    await expect(panel.getByText(row.name, { exact: true })).toBeVisible();
  for (const row of [archived, collaboration])
    await expect(panel.getByText(row.name, { exact: true })).toHaveCount(0);
  const loadMore = panel.getByRole("button", { name: "Load more" });
  const firstPage = queries.find(
    (url) =>
      url.pathname === "/api/chats" &&
      !url.searchParams.has("project_id") &&
      !url.searchParams.has("agent_id") &&
      url.searchParams.get("without_project") !== "true",
  );
  expect(firstPage).toBeDefined();
  const pageSize = Number(firstPage!.searchParams.get("limit"));
  const secondPageFirst = chats.filter(
    (row) => !row.archived && !row.collaboration,
  )[pageSize];
  await expect(
    panel.getByText(secondPageFirst.name, { exact: true }),
  ).toHaveCount(0);
  await loadMore.click();
  await expect(
    panel.getByText(secondPageFirst.name, { exact: true }),
  ).toBeVisible();
  await expect
    .poll(() =>
      queries.some(
        (url) =>
          url.pathname === "/api/chats" &&
          Number(url.searchParams.get("offset")) > 0,
      ),
    )
    .toBe(true);
  await expect(panel.getByText(photo.name, { exact: true })).toHaveCount(1);

  await lookup.fill("page item");
  await expect(
    panel.getByText(`Search hit in ${pages[0].name}`, { exact: true }),
  ).toBeVisible();
  const searchPage = queries.find(
    (url) =>
      url.pathname === "/api/chats/search" &&
      url.searchParams.get("q") === "page item",
  );
  expect(searchPage).toBeDefined();
  const searchSize = Number(searchPage!.searchParams.get("limit"));
  const nextHit = panel.getByText(`Search hit in ${pages[searchSize].name}`, {
    exact: true,
  });
  await expect(nextHit).toHaveCount(0);
  await loadMore.click();
  await expect(nextHit).toBeVisible();
  await expect(
    panel.getByText(`Search hit in ${pages[0].name}`, { exact: true }),
  ).toHaveCount(1);
  await lookup.fill("");
  await expect(panel.getByText(photo.name, { exact: true })).toBeVisible();

  await agent.selectOption({ label: "Research" });
  await project.selectOption({ label: "Photos" });
  await expect(panel.getByText(photo.name, { exact: true })).toBeVisible();
  for (const row of [standalone, otherAgent, album, archived, collaboration])
    await expect(panel.getByText(row.name, { exact: true })).toHaveCount(0);
  await lookup.fill("photo");
  await expect(
    panel.getByText(`Search hit in ${photo.name}`, { exact: true }),
  ).toBeVisible();
  expect(
    queries.some(
      (url) =>
        url.pathname === "/api/chats/search" &&
        url.searchParams.get("agent_id") === "research" &&
        url.searchParams.get("project_id") === "photos" &&
        url.searchParams.get("view") === "user" &&
        url.searchParams.get("archived") === "false",
    ),
  ).toBe(true);
  await expect(
    panel.getByText(`Search hit in ${collaboration.name}`, { exact: true }),
  ).toHaveCount(0);
  await archiveFilter.selectOption({ label: "Archived" });
  await expect(
    panel.getByText(`Search hit in ${archived.name}`, { exact: true }),
  ).toBeVisible();
  await expect(
    panel.getByText(`Search hit in ${photo.name}`, { exact: true }),
  ).toHaveCount(0);
  await lookup.fill("");
  await expect(panel.getByText(archived.name, { exact: true })).toBeVisible();
  await archiveFilter.selectOption({ label: "Active" });
  await project.selectOption({ label: "Standalone chats" });
  await expect(panel.getByText(standalone.name, { exact: true })).toBeVisible();
  await expect(panel.getByText(photo.name, { exact: true })).toHaveCount(0);
  await project.selectOption({ label: "Photos" });
  await lookup.fill("catalog");
  await panel.getByText(`Search hit in ${photo.name}`, { exact: true }).click();
  await expect(page).toHaveURL(new RegExp(`/chats/${photo.id}$`));
  await expect(
    page.getByText(`Saved history for ${photo.name}`, { exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Chat Agent", exact: true }),
  ).toContainText("Research");
  if (await panel.isVisible()) await search.click();
  const group = sidebar.getByRole("button", { name: "Photos", exact: true });
  if ((await group.getAttribute("aria-expanded")) !== "true")
    await group.click();
  await sidebar
    .getByRole("button", { name: `Pinned ${photo.name}`, exact: true })
    .click();
  await expect(page).toHaveURL(new RegExp(`/chats/${photo.id}$`));
  await expect(
    page.getByText(`Saved history for ${photo.name}`, { exact: true }),
  ).toBeVisible();
});

for (const row of [standalone, photo]) {
  test(`switches the Agent in place for ${row.name} and preserves the original Turn`, async ({
    page,
  }) => {
    await page.setViewportSize({ width: 1440, height: 900 });
    const state = {
      rejectOnce: "Agent switch blocked: unresolved_approval",
      blockers: [] as string[],
    };
    await mockDiscoveryServer(page, state);
    await page.goto(`/chats/${row.id}`);
    const selector = page.getByRole("button", {
      name: "Chat Agent",
      exact: true,
    });
    const transcript = page.locator('[data-rua-chat-role="assistant"]');
    const input = page.locator(".rua-owned-composer textarea");
    await expect(selector).toContainText("Research");
    await expect(transcript).toContainText("Original Turn Agent");
    await input.fill("Keep this pending input");
    const pick = async () => {
      await selector.click();
      await page
        .getByRole("button", { name: /Default.*Switch at stable boundary/ })
        .click();
      await page
        .getByRole("dialog")
        .getByRole("button", { name: "Switch Agent", exact: true })
        .click();
    };
    await pick();
    await expect(
      page.getByText("Resolve the pending approval before switching Agent", {
        exact: true,
      }),
    ).toBeVisible();
    await expect(selector).toContainText("Research");
    await expect(input).toHaveValue("Keep this pending input");
    state.blockers = ["active_acp_binding"];
    await selector.click();
    await expect(
      page.getByRole("button", { name: /Default.*Exit ACP/ }),
    ).toBeDisabled();
    await selector.click();
    state.blockers = [];

    await pick();
    await expect(selector).toContainText("Default");
    await expect(input).toHaveValue("Keep this pending input");
    await expect(transcript).toContainText("Original Turn Agent");
    await expect(page).toHaveURL(new RegExp(`/chats/${row.id}$`));
    const sidebar = page.locator("#desktop-navigation");
    await sidebar
      .getByRole("button", { name: "Search chats", exact: true })
      .click();
    const discovery = sidebar.getByRole("region", { name: "Chat discovery" });
    const agent = discovery.getByRole("combobox", { name: "Agent filter" });
    await agent.selectOption({ label: "Research" });
    await expect(discovery.getByText(row.name, { exact: true })).toHaveCount(0);
    await agent.selectOption({ label: "Default" });
    await expect(discovery.getByText(row.name, { exact: true })).toBeVisible();
    await page.reload();
    await expect(selector).toContainText("Default");
    await expect(transcript).toContainText("Original Turn Agent");
    await expect(input).toHaveValue("Keep this pending input");
    await page.goto(`/chats/${collaboration.id}`);
    await selector.click();
    await expect(
      page.getByRole("button", { name: /Default.*fixed executor/ }),
    ).toBeDisabled();
  });
}

test("two windows suppress an open Chat and present one notification that returns to its turn", async ({
  page,
  context,
}) => {
  const second = await context.newPage();
  const shown: { page: Page; title: string }[] = [];
  for (const view of [page, second]) {
    await mockDiscoveryServer(view);
    await view.exposeFunction("recordFixtureNotification", (title: string) =>
      shown.push({ page: view, title }),
    );
    await view.addInitScript((active) => {
      const harness = window as Window & {
        recordFixtureNotification(title: string): void;
      };
      Object.defineProperty(document, "hasFocus", { value: () => active });
      Object.defineProperty(document, "visibilityState", {
        get: () => (active ? "visible" : "hidden"),
      });
      window.addEventListener("fixture-background", () => {
        active = false;
        document.dispatchEvent(new Event("visibilitychange"));
      });
      const viewers = new BroadcastChannel("rua:chat-viewer-v1");
      viewers.onmessage = ({ data }) => {
        if (data?.kind === "viewer")
          document.documentElement.dataset.viewedChat = data.chatAddress ?? "";
      };
      const NativeSource = window.EventSource;
      window.EventSource = class extends NativeSource {
        constructor(url: string | URL, init?: EventSourceInit) {
          super(url, init);
          if (String(url).endsWith("/platform/user-notifications/events")) {
            document.documentElement.dataset.notificationListener = "ready";
            window.addEventListener("fixture-notification", (event) => {
              this.dispatchEvent(
                new MessageEvent("user_notification", {
                  data: JSON.stringify((event as CustomEvent).detail),
                }),
              );
            });
          }
        }
      };
      class FixtureNotification {
        static permission = "granted";
        onclick?: () => void;
        constructor(title: string) {
          harness.recordFixtureNotification(title);
          window.addEventListener("fixture-click", () => this.onclick?.(), {
            once: true,
          });
        }
        close() {}
      }
      Object.defineProperty(window, "Notification", {
        value: FixtureNotification,
      });
    }, view === page);
  }

  await page.goto(`/console/chats/${photo.id}`);
  await expect(page.getByText(`Saved history for ${photo.name}`)).toBeVisible();
  await second.goto("/console/settings");
  await expect(second.locator("html")).toHaveAttribute(
    "data-notification-listener",
    "ready",
  );
  await expect(second.locator("html")).toHaveAttribute(
    "data-viewed-chat",
    `chats/${photo.id}`,
  );

  const turn = `turn-${photo.id}`;
  const uri = `rua://chats/${photo.id}?turn=${turn}&from=notification#result`;
  const send = (view: Page, title: string, rua_uri?: string) =>
    view.evaluate(
      (payload) => {
        window.dispatchEvent(
          new CustomEvent("fixture-notification", { detail: payload }),
        );
      },
      {
        title,
        rua_uri,
        sender: { id: "previous-agent", name: "Previous Agent" },
        target_device_id: "desktop",
      },
    );
  await Promise.all([
    send(page, "Already open", uri),
    send(second, "Already open", uri),
  ]);
  await send(second, "Unrelated");
  await expect
    .poll(() => shown.map((item) => item.title))
    .toEqual(["Unrelated"]);

  await page.evaluate(() =>
    window.dispatchEvent(new Event("fixture-background")),
  );
  await expect(second.locator("html")).toHaveAttribute("data-viewed-chat", "");
  await Promise.all([
    send(page, "Completed", uri),
    send(second, "Completed", uri),
  ]);
  await expect
    .poll(() => shown.filter((item) => item.title === "Completed").length)
    .toBe(1);
  const presenting = shown.find((item) => item.title === "Completed")!.page;
  await presenting.evaluate(() =>
    window.dispatchEvent(new Event("fixture-click")),
  );
  await expect(presenting).toHaveURL(
    new RegExp(
      `/console/chats/${photo.id}\\?turn=${turn}&from=notification#result$`,
    ),
  );
  await expect(
    presenting.locator(".rua-chat-message--search-jump"),
  ).toContainText(`Saved history for ${photo.name}`);
  expect(shown.map((item) => item.title)).toEqual(["Unrelated", "Completed"]);
});
