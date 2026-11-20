import { expect, test } from "@playwright/test";
import { mockConsoleTransport } from "./fixtures/consoleTransport";

for (const operation of ["submit", "submit-fails", "upload", "acp"] as const) {
  test(`restores the draft execution device before ${operation}`, async ({
    page,
  }) => {
    const model = { provider_id: "fixture", model: "chosen-model" };
    const mediaId = "00000000-0000-4000-8000-000000000912";
    const writes: string[] = [];
    const requests: Record<string, unknown>[] = [];
    let chatId = "";
    let accepted = false;
    let releaseSubmission!: () => void;
    const submissionGate = new Promise<void>((resolve) => {
      releaseSubmission = resolve;
    });
    await page.addInitScript((model) => {
      localStorage.setItem("language", "en");
      localStorage.setItem(
        "rua_last_conversation_model",
        JSON.stringify(model),
      );
    }, model);
    await mockConsoleTransport(page);
    await page.route("**/api/**", async (route) => {
      const request = route.request();
      const url = new URL(request.url());
      if (!url.pathname.startsWith("/api/")) return route.continue();
      const path = url.pathname.slice(4);
      const json = (value: unknown) => route.fulfill({ json: value });
      if (request.method() !== "GET" && path.startsWith("/chats/"))
        writes.push(path);
      if (path === "/auth/status") return json({ enabled: false });
      if (path === "/agents")
        return json({
          agents: [{ id: "default", name: "Default", enabled: true }],
        });
      if (path === "/projects") return json({ projects: [] });
      if (path === "/project-sidebar")
        return json({ revision: "1", sort: "manual", items: {} });
      if (path === "/sync/status") return json({ enabled: true, paired: true });
      if (path === "/sync/devices/status")
        return json({ revocation_state: "active", is_revoked: false });
      if (path === "/sync/devices")
        return json({
          devices: [
            {
              device_id: "phone",
              friendly_name: "Phone",
              is_self: true,
              connectivity_state: "connected",
            },
            {
              device_id: "desktop",
              friendly_name: "Desktop",
              is_self: false,
              connectivity_state: "connected",
            },
          ],
        });
      if (path === "/models/active") return json({ default_llm: model });
      if (path === "/models")
        return json([
          {
            id: "fixture",
            name: "Fixture",
            models: [{ id: model.model, name: model.model }],
            extra_models: [],
          },
        ]);
      if (path === "/chats")
        return json(
          accepted
            ? [{ id: chatId, agent_id: "default", name: "First input" }]
            : [],
        );
      if (chatId && path === `/chats/${chatId}/console/submit`) {
        const body = request.postDataJSON();
        requests.push(body);
        await submissionGate;
        if (operation === "submit-fails")
          return json({
            accepted: false,
            mode: "rejected",
            detail: "Target rejected the first input",
          });
        accepted = true;
        return json({
          accepted: true,
          mode: "started",
          chat_id: chatId,
          input_id: "input-1",
          submission_id: body.submission_id,
          device_delivery_id: "delivery-1",
          device_delivery_sequence: 1,
          target_device_id: "desktop",
        });
      }
      if (
        chatId &&
        path === `/chats/${chatId}/media` &&
        request.method() === "POST"
      )
        return json({
          media_ref: `media/${mediaId}`,
          media_id: mediaId,
          mime_type: "text/plain",
          byte_size: 5,
          original_filename: "note.txt",
        });
      if (path.endsWith("/acp/runners"))
        return json([
          {
            key: "test",
            command: "test-runner",
            enabled: true,
            installed: true,
          },
        ]);
      if (path.endsWith("/acp/sessions")) return json([]);
      if (chatId && path === `/chats/${chatId}`) {
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
          messages: [],
          meta: {
            authority_placement: {
              device_id: "desktop",
              authority_epoch: "0000000000000002000000000000000000000000",
            },
          },
        });
      }
      if (path.includes("/approvals") || path.includes("/permissions"))
        return json({ pending: [], resolved: [] });
      if (
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
    const device = composer.locator(".rua-chat-device-chip");
    await device.click();
    await page
      .getByRole("dialog")
      .getByRole("button", { name: /Desktop/i })
      .click();
    await expect(device).toContainText("Desktop");
    await page.reload();
    await expect(device).toContainText("Desktop");
    expect(writes).toEqual([]);

    if (operation === "upload") {
      await composer.locator('input[type="file"]').setInputFiles({
        name: "note.txt",
        mimeType: "text/plain",
        buffer: Buffer.from("hello"),
      });
      await expect(composer).toContainText("note.txt");
      expect(writes).toEqual([`/chats/${chatId}/media`]);
    }
    if (operation === "acp") {
      await page
        .getByRole("main")
        .getByRole("button", { name: "More", exact: true })
        .click();
      await page
        .getByRole("menuitem", { name: "Enter ACP pass-through", exact: true })
        .click();
      const dialog = page.locator(".rua-acp-enter-dialog");
      await dialog.getByRole("button", { name: "New", exact: true }).click();
      await expect(dialog).toHaveCount(0);
      expect(writes).toEqual([]);
    }

    await input.fill("first input");
    await input.press("Enter");
    await expect.poll(() => requests.length).toBe(1);
    expect(requests[0]).toMatchObject({
      chat_id: chatId,
      agent_id: "default",
      target_device_id: "desktop",
      model,
    });
    if (operation === "acp")
      expect(requests[0].initial_acp).toMatchObject({ runner: "test" });
    if (operation === "upload")
      expect(JSON.stringify(requests[0].input)).toContain(`media/${mediaId}`);
    await expect(page.locator(".rua-chat-message--user")).toContainText(
      "first input",
    );
    await expect(input).toHaveValue("");
    await expect(page.locator(".rua-chat-remote-pending")).toContainText(
      /Delivering|Sending/,
    );
    releaseSubmission();

    if (operation === "submit-fails") {
      await expect(input).toHaveValue("first input");
      await expect(page.locator(".rua-chat-remote-pending")).toHaveCount(0);
      await page.reload();
      await expect(input).toHaveValue("first input");
      await expect(device).toContainText("Desktop");
    } else {
      await expect(page.locator(".rua-chat-remote-pending")).toContainText(
        "Delivered, waiting for result",
      );
      await expect(page.locator(".rua-chat-message--user")).toHaveCount(1);
    }
    expect(writes).toEqual([
      ...(operation === "upload" ? [`/chats/${chatId}/media`] : []),
      `/chats/${chatId}/console/submit`,
    ]);
  });
}
