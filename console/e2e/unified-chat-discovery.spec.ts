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

async function mockDiscoveryServer(page: Page) {
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
        const rows = chats.filter(
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
      const hit = /^\/chats\/([^/]+)(?:\/metadata)?$/.exec(path);
      const row = hit && chats.find((item) => item.id === hit[1]);
      if (row) {
        if (path.endsWith("/metadata")) return json(row);
        return json({
          ...row,
          title: row.name,
          model,
          messages: [
            {
              id: `message-${row.id}`,
              turn_id: `turn-${row.id}`,
              role: "assistant",
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
    page.getByRole("button", { name: "Project Chat Agent", exact: true }),
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
