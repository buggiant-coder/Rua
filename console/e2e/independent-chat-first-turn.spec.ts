import { expect, test } from "@playwright/test";
import { mockConsoleTransport } from "./fixtures/consoleTransport";

test("a staged attachment and accepted first turn reopen at the original Chat ID", async ({
  page,
}) => {
  const mediaId = "00000000-0000-4000-8000-000000000211";
  const model = { provider_id: "fixture", model: "first-turn-model" };
  const reply = "The attachment arrived.";
  let chatId = "";
  let accepted = false;
  let historyReads = 0;
  const submitted: Record<string, unknown>[] = [];
  const writes: string[] = [];

  await page.addInitScript(
    ({ model }) => {
      localStorage.setItem("language", "en");
      localStorage.setItem(
        "rua_last_conversation_model",
        JSON.stringify(model),
      );
    },
    { model },
  );
  await mockConsoleTransport(page);
  await page.route("**/api/**", async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname.slice(4);
    if (!new URL(request.url()).pathname.startsWith("/api/"))
      return route.continue();
    const json = (value: unknown) => route.fulfill({ json: value });
    if (request.method() !== "GET" && path.startsWith("/chats"))
      writes.push(path);
    if (path === "/auth/status") return json({ enabled: false });
    if (path === "/agents")
      return json({
        agents: [{ id: "default", name: "Default", enabled: true }],
      });
    if (path === "/projects") return json({ projects: [] });
    if (path === "/project-sidebar")
      return json({ revision: "1", sort: "manual", items: {} });
    if (path === "/sync/devices") return json({ devices: [] });
    if (path === "/models/active") return json({ default_llm: model });
    if (path === "/chats")
      return json(
        accepted
          ? [{ id: chatId, name: "First input", agent_id: "default" }]
          : [],
      );
    if (chatId && path === `/chats/${chatId}/media`) {
      expect(request.postData()).toContain('name="agent_id"');
      return json({
        media_ref: `media/${mediaId}`,
        media_id: mediaId,
        byte_size: 5,
        mime_type: "text/plain",
        original_filename: "note.txt",
      });
    }
    if (chatId && path === `/chats/${chatId}/console/submit`) {
      const body = request.postDataJSON();
      submitted.push(body);
      if (submitted.length === 1)
        return json({ accepted: false, mode: "rejected", detail: "Try again" });
      accepted = true;
      return json({
        accepted: true,
        mode: "started",
        submission_id: body.submission_id,
        chat_id: chatId,
      });
    }
    if (chatId && path === `/chats/${chatId}`) {
      ++historyReads;
      if (!accepted)
        return route.fulfill({
          status: 404,
          json: { detail: "Chat not found" },
        });
      return json({
        id: chatId,
        agent_id: "default",
        project_id: null,
        model,
        status: "idle",
        messages: [
          {
            id: "user-first",
            role: "user",
            turn_id: "turn-first",
            content: [
              { type: "text", text: "First input" },
              {
                type: "file",
                media_ref: `media/${mediaId}`,
                filename: "note.txt",
              },
            ],
          },
          {
            id: "assistant-first",
            role: "assistant",
            turn_id: "turn-first",
            content: [{ type: "text", text: reply }],
          },
        ],
      });
    }
    if (path.endsWith("/metadata"))
      return json({ id: chatId, agent_id: "default" });
    if (
      path === "/models" ||
      path.includes("/channels") ||
      path.includes("/templates") ||
      path.includes("/providers")
    )
      return json([]);
    return json({});
  });

  await page.goto("/");
  await expect(page).toHaveURL(/\/chats\/[a-f0-9-]{36}$/);
  chatId = new URL(page.url()).pathname.split("/").at(-1)!;
  const composer = page.locator(".rua-owned-composer");
  const input = composer.locator("textarea");
  await input.fill("First input");
  await composer.locator('input[type="file"]').setInputFiles({
    name: "note.txt",
    mimeType: "text/plain",
    buffer: Buffer.from("hello"),
  });
  await expect.poll(() => writes).toEqual([`/chats/${chatId}/media`]);
  expect(historyReads).toBe(0);

  await input.press("Enter");
  await expect.poll(() => submitted.length).toBe(1);
  await expect(input).toHaveValue(/First input/);
  expect(historyReads).toBe(0);
  await input.press("Enter");
  await expect.poll(() => submitted.length).toBe(2);
  expect(submitted[1]).toMatchObject({
    chat_id: chatId,
    agent_id: "default",
    model,
  });
  expect(JSON.stringify(submitted[1].input)).toContain(`media/${mediaId}`);
  await expect(page.getByText(reply, { exact: true })).toBeVisible();
  const readsBeforeReload = historyReads;
  await page.reload();
  await expect(page).toHaveURL(new RegExp(`/chats/${chatId}$`));
  await expect(page.getByText(reply, { exact: true })).toBeVisible();
  expect(historyReads).toBeGreaterThan(readsBeforeReload);
  expect(writes).toEqual([
    `/chats/${chatId}/media`,
    `/chats/${chatId}/console/submit`,
    `/chats/${chatId}/console/submit`,
  ]);
});

test("New chat restores one draft per Project or standalone group using the current Agent Selector", async ({
  page,
}) => {
  const projects = ["Photos", "Albums"].map((name) => ({
    project_id: name.toLowerCase(),
    display_name: name,
    display_path: `/fixture/${name}`,
    resource_device_id: "desktop",
    lifecycle: "active",
  }));
  const agents = ["Default", "Research"].map((name) => ({
    id: name.toLowerCase(),
    name,
    enabled: true,
  }));
  const savedId = "00000000-0000-4000-8000-000000000212";
  const mediaId = "00000000-0000-4000-8000-000000000213";
  const model = { provider_id: "fixture", model: "basic" };
  const savedHistory = {
    id: savedId,
    agent_id: "default",
    project_id: "photos",
    title: "Saved photo chat",
    status: "idle",
    model,
    messages: [
      {
        id: "saved-reply",
        role: "assistant",
        turn_id: "saved-turn",
        content: [{ type: "text", text: "Saved photo reply" }],
      },
    ],
    meta: {
      chat_agent_state: {
        selection: { agent_id: "default", revision: 1 },
        selected_agent_available: true,
        switchable: true,
        blockers: [],
      },
    },
  };
  const histories = new Map([[savedId, savedHistory]]);
  const writes: string[] = [];
  const submitted: Record<string, unknown>[] = [];

  await page.setViewportSize({ width: 1440, height: 900 });
  await page.addInitScript(() => {
    localStorage.setItem("language", "en");
    localStorage.setItem("rua_sider_collapsed", "false");
  });
  await mockConsoleTransport(page);
  await page.route(
    (url) => url.pathname.startsWith("/api/"),
    async (route) => {
      const request = route.request();
      const path = new URL(request.url()).pathname.slice(4);
      const json = (value: unknown) => route.fulfill({ json: value });
      if (path.startsWith("/chats/") && request.method() !== "GET")
        writes.push(path);
      if (path === "/auth/status") return json({ enabled: false });
      if (path === "/agents") return json({ agents });
      if (path === "/projects") return json({ projects });
      const project = projects.find(
        (item) => path === `/projects/${item.project_id}`,
      );
      if (project) return json(project);
      if (path === "/project-sidebar")
        return json({ revision: "1", sort: "manual", items: {} });
      if (path === "/sync/devices") return json({ devices: [] });
      if (path === "/models/active") return json({ default_llm: model });
      if (path === "/models")
        return json([
          {
            id: "fixture",
            name: "Fixture",
            require_api_key: false,
            models: [
              { id: "basic", name: "Basic" },
              { id: "detail", name: "Detailed" },
            ],
          },
        ]);
      const historyId = /^\/chats\/([^/]+)(?:\/metadata)?$/.exec(path)?.[1];
      const history = historyId && histories.get(historyId);
      if (history) return json(history);
      if (path.endsWith("/chats")) return json([]);
      if (/^\/chats\/[^/]+$/.test(path))
        return route.fulfill({
          status: 404,
          json: { detail: "Draft has no history" },
        });
      if (path.endsWith("/media") && request.method() === "POST")
        return json({
          media_id: mediaId,
          media_ref: `media/${mediaId}`,
          original_filename: "draft-note.txt",
          mime_type: "text/plain",
          byte_size: 4,
        });
      if (path.endsWith("/console/submit")) {
        const body = request.postDataJSON();
        submitted.push(body);
        if (submitted.length === 1)
          return json({
            accepted: false,
            mode: "rejected",
            detail: "Try again",
          });
        const id = path.split("/")[2];
        histories.set(id, {
          ...savedHistory,
          id,
          project_id: body.project_id,
          agent_id: body.agent_id,
          model: body.model,
          meta: {
            chat_agent_state: {
              ...savedHistory.meta.chat_agent_state,
              selection: { agent_id: body.agent_id, revision: 1 },
            },
          },
          messages: [
            {
              id: "accepted-reply",
              role: "assistant",
              turn_id: "accepted-turn",
              content: [{ type: "text", text: "Photos accepted" }],
            },
          ],
        });
        return json({
          accepted: true,
          mode: "started",
          chat_id: id,
          submission_id: body.submission_id,
        });
      }
      if (path.endsWith("/tasks")) return json({ tasks: [] });
      if (
        path.includes("/templates") ||
        path.includes("/channels") ||
        path.includes("/providers")
      )
        return json([]);
      return json({});
    },
  );

  const input = page.locator(".rua-owned-composer textarea");
  const header = page.locator(".rua-owned-chat-header");
  const projectAgent = header.getByRole("button", {
    name: "Chat Agent",
    exact: true,
  });
  const newIn = (group: string) =>
    page
      .locator("#desktop-navigation")
      .getByRole("button", { name: `New chat in ${group}`, exact: true })
      .click();
  await page.goto("/");
  await input.fill("Standalone pending input");
  const standaloneUrl = page.url();
  await newIn("Photos");
  await expect(projectAgent).toContainText("Default");
  await input.fill("Photos pending input");
  const photosUrl = page.url();
  await page.locator('.rua-owned-composer input[type="file"]').setInputFiles({
    name: "draft-note.txt",
    mimeType: "text/plain",
    buffer: Buffer.from("note"),
  });
  await expect(page.locator(".rua-owned-composer")).toContainText(
    "draft-note.txt",
  );
  await page.getByRole("button", { name: /^Model and thinking:/i }).click();
  await page.getByRole("button", { name: /^Model:/i }).click();
  await page.getByRole("button", { name: "Detailed", exact: true }).click();
  await page.getByRole("button", { name: "High", exact: true }).click();

  await newIn("Albums");
  await expect(input).toHaveValue("");
  await input.fill("Albums pending input");
  const albumsUrl = page.url();
  expect(new Set([standaloneUrl, photosUrl, albumsUrl]).size).toBe(3);
  await newIn("Standalone chats");
  await expect(page).toHaveURL(standaloneUrl);
  await expect(input).toHaveValue("Standalone pending input");
  await header
    .getByRole("button", { name: "Chat Agent", exact: true })
    .click();
  await page
    .getByRole("button", { name: /Research/ })
    .click();
  await header.getByRole("button", { name: "New chat", exact: true }).click();
  await expect(page).toHaveURL(standaloneUrl);
  await expect(input).toHaveValue("Standalone pending input");
  await expect(
    header.getByRole("button", { name: "Chat Agent", exact: true }),
  ).toContainText("Research");

  await newIn("Photos");
  await expect(page).toHaveURL(photosUrl);
  await expect(projectAgent).toContainText("Research");
  await expect(input).toHaveValue("Photos pending input");
  await expect(page.locator(".rua-owned-composer")).toContainText(
    "draft-note.txt",
  );
  await expect(
    page.getByRole("button", { name: /^Model and thinking:/i }),
  ).toContainText("Detailed");
  expect(writes).toEqual([`${new URL(photosUrl).pathname}/media`]);
  await input.press("Enter");
  await expect.poll(() => submitted.length).toBe(1);
  expect(submitted[0]).toMatchObject({
    agent_id: "research",
    project_id: "photos",
    model: {
      provider_id: "fixture",
      model: "detail",
      thinking_strength: "high",
    },
  });
  expect(JSON.stringify(submitted[0].input)).toContain(`media/${mediaId}`);
  await expect(input).toHaveValue(/Photos pending input/);
  await page.reload();
  await expect(input).toHaveValue("Photos pending input");

  await page.goto(`/chats/${savedId}`);
  await expect(
    page.getByText("Saved photo reply", { exact: true }),
  ).toBeVisible();
  await expect(projectAgent).toContainText("Default");
  await input.fill("Unsent text for the existing photo chat");
  await header
    .getByRole("button", { name: "New Project Chat", exact: true })
    .click();
  await expect(page).toHaveURL(photosUrl);
  await expect(projectAgent).toContainText("Research");
  await expect(input).toHaveValue("Photos pending input");
  await input.press("Enter");
  await expect(
    page.getByText("Photos accepted", { exact: true }),
  ).toBeVisible();
  expect(submitted[1]).toMatchObject({
    chat_id: submitted[0].chat_id,
    agent_id: "research",
    project_id: "photos",
    model: submitted[0].model,
  });
  expect(JSON.stringify(submitted[1].input)).toContain(`media/${mediaId}`);
  await input.fill("Unsent text for the newly saved photo chat");

  await newIn("Albums");
  await expect(page).toHaveURL(albumsUrl);
  await expect(input).toHaveValue("Albums pending input");
  await newIn("Photos");
  await expect(page).not.toHaveURL(photosUrl);
  await expect(input).toHaveValue("");
  await newIn("Standalone chats");
  await expect(page).toHaveURL(standaloneUrl);
  await expect(input).toHaveValue("Standalone pending input");
  await page.goto(`/chats/${savedId}`);
  await expect(input).toHaveValue("Unsent text for the existing photo chat");
  await page.goto(photosUrl);
  await expect(input).toHaveValue("Unsent text for the newly saved photo chat");
});
