import { expect, test } from "@playwright/test";

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
  await page.routeWebSocket("**/api/console/transport", (socket) => {
    socket.onMessage((message) => {
      const frame = JSON.parse(String(message));
      if (frame.type === "auth")
        socket.send(JSON.stringify({ type: "ready", version: 1 }));
      if (frame.type !== "open") return;
      const snapshot =
        frame.path.includes("/approvals/") ||
        frame.path.includes("/permissions");
      socket.send(
        JSON.stringify({
          type: "head",
          id: frame.id,
          status: 200,
          content_type: snapshot ? "application/json" : "text/event-stream",
        }),
      );
      if (snapshot) {
        socket.send(
          JSON.stringify({
            type: "data",
            id: frame.id,
            data: Buffer.from('{"pending":[],"resolved":[]}').toString(
              "base64",
            ),
          }),
        );
        socket.send(JSON.stringify({ type: "end", id: frame.id }));
      }
    });
  });
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
  await composer
    .locator('input[type="file"]')
    .setInputFiles({
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
