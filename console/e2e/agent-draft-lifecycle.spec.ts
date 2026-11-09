import { expect, test } from "@playwright/test";

for (const operation of ["submit", "submit-fails", "upload", "acp"] as const) {
  test(`draft target stays local until ${operation} creates on the target`, async ({
    page,
  }) => {
    const chatId = "00000000-0000-4000-8000-000000000911";
    const calls: { path: string; method: string; body: unknown }[] = [];
    let created = false;
    let releaseCreation!: () => void;
    let releaseSubmission!: () => void;
    const creationGate = new Promise<void>((resolve) => {
      releaseCreation = resolve;
    });
    const submissionGate = new Promise<void>((resolve) => {
      releaseSubmission = resolve;
    });
    const model = { provider_id: "fixture", model: "chosen-model" };
    await page.addInitScript(
      ({ chatId, model }) => {
        localStorage.setItem("language", "en");
        localStorage.setItem("rua_draft_chat_id_default", chatId);
        sessionStorage.setItem(
          `rua_draft_chat_model_default:${chatId}`,
          JSON.stringify(model),
        );
        sessionStorage.setItem(
          `rua_draft_chat_device_default:${chatId}`,
          "desktop",
        );
        const original = window.fetch.bind(window);
        window.fetch = (input, init) => {
          const url =
            typeof input === "string"
              ? input
              : input instanceof URL
              ? input.href
              : input.url;
          if (url.includes("/console/live"))
            return Promise.resolve(
              new Response(
                new ReadableStream({
                  start(controller) {
                    controller.enqueue(
                      new TextEncoder().encode(": connected\n\n"),
                    );
                  },
                }),
                { headers: { "Content-Type": "text/event-stream" } },
              ),
            );
          return original(input, init);
        };
      },
      { chatId, model },
    );
    await page.route("**/api/**", async (route) => {
      if (!new URL(route.request().url()).pathname.startsWith("/api/"))
        return route.continue();
      const path = new URL(route.request().url()).pathname.replace(
        /^\/api/,
        "",
      );
      const method = route.request().method();
      const body = route
        .request()
        .headers()
        ["content-type"]?.includes("application/json")
        ? route.request().postDataJSON()
        : null;
      calls.push({ path, method, body });
      const json = (value: unknown) => route.fulfill({ json: value });
      if (path === "/platform/lifecycle" || path.endsWith("/events"))
        return route.fulfill({
          contentType: "text/event-stream",
          body: ": connected\n\n",
        });
      if (path === "/auth/status")
        return json({ enabled: false, has_users: false });
      if (path === "/agents")
        return json({
          agents: [
            {
              id: "default",
              name: "Default",
              enabled: true,
              workspace_dir: "/test",
            },
          ],
        });
      if (path === "/sync/status")
        return json({ enabled: true, paired: true, peer_count: 1 });
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
      if (path.endsWith("/materialize")) {
        if (operation === "submit" || operation === "submit-fails")
          await creationGate;
        if (operation === "submit-fails")
          return route.fulfill({
            status: 503,
            json: { detail: "target offline" },
          });
        expect(body).toEqual({ device_id: "desktop", model });
        created = true;
        return json({
          model,
          meta: {
            authority_placement: {
              device_id: "desktop",
              authority_epoch: "0000000000000002000000000000000000000000",
            },
          },
        });
      }
      if (path.endsWith("/console/submit")) {
        if (operation === "submit") await submissionGate;
        expect(created).toBe(true);
        return json({
          accepted: true,
          mode: "started",
          chat_id: chatId,
          input_id: "input-1",
          submission_id: "submission-1",
          device_delivery_id: "delivery-1",
          device_delivery_sequence: 1,
          target_device_id: "desktop",
        });
      }
      if (path.endsWith("/media") && method === "POST") {
        expect(created).toBe(true);
        return json({
          media_ref: "media/00000000-0000-4000-8000-000000000912",
          media_id: "00000000-0000-4000-8000-000000000912",
          mime_type: "text/plain",
          byte_size: 5,
          authority_epoch: "0000000000000002000000000000000000000000",
          original_filename: "note.txt",
        });
      }
      if (path.endsWith("/acp/runners"))
        return json([
          {
            key: "test",
            name: "Test Runner",
            enabled: true,
            installed: true,
            availability: "available",
          },
        ]);
      if (path.endsWith("/acp/sessions")) return json([]);
      if (path.endsWith("/acp/enter")) {
        expect(created).toBe(true);
        return json({
          message: "entered",
          status: {
            active: true,
            runner: "test",
            session_id: "acp-1",
            target_device_id: "desktop",
          },
        });
      }
      if (path.endsWith(`/chats/${chatId}`))
        return json({
          status: "idle",
          messages: [],
          model,
          ...(created
            ? {
                meta: {
                  authority_placement: {
                    device_id: "desktop",
                    authority_epoch: "0000000000000002000000000000000000000000",
                  },
                },
              }
            : {}),
        });
      if (path.endsWith("/chats")) return json({ chats: [], total: 0 });
      if (path.includes("approvals") || path.includes("permissions"))
        return json({ pending: [], resolved: [] });
      if (path.endsWith("/activity"))
        return json({ status: "idle", active_turn: null });
      if (path.includes("/channels")) return json([]);
      if (path.includes("/tasks")) return json({ tasks: [] });
      if (path.endsWith("/status")) return json({ active: false });
      if (
        path.includes("/skills") ||
        path.includes("/templates") ||
        path.includes("/runners")
      )
        return json([]);
      return json({});
    });
    await page.goto(`/agents/default/chats/${chatId}`);
    const input = page.locator(".rua-owned-composer textarea");
    await expect(input).toBeVisible();
    await expect(page.locator(".rua-owned-composer")).toContainText(
      "Desktop",
    );
    expect(
      calls.filter(
        (call) =>
          call.path.endsWith("/materialize") ||
          call.path.endsWith("/authority-placement"),
      ),
    ).toEqual([]);
    let suffix: string;
    if (operation === "submit" || operation === "submit-fails") {
      await input.fill("first input");
      await input.press("Enter");
      await expect(page.locator(".rua-chat-message--user")).toContainText(
        "first input",
      );
      await expect(input).toHaveValue("");
      await expect(page.locator(".rua-owned-composer")).toContainText(
        "Desktop",
      );
      await expect(page.locator(".rua-chat-remote-pending")).toContainText(
        /Delivering|Sending/,
      );
      expect(calls.some((call) => call.path.endsWith("/console/submit"))).toBe(
        false,
      );
      releaseCreation();
      if (operation === "submit-fails") {
        await expect(input).toHaveValue("first input");
        await expect(page.locator(".rua-chat-remote-pending")).toHaveCount(0);
        expect(
          calls.some((call) => call.path.endsWith("/console/submit")),
        ).toBe(false);
        return;
      }
      await expect
        .poll(() => calls.some((call) => call.path.endsWith("/console/submit")))
        .toBe(true);
      await expect(page.locator(".rua-chat-message--user")).toContainText(
        "first input",
      );
      await expect(page.locator(".rua-owned-composer")).toContainText(
        "Desktop",
      );
      releaseSubmission();
      await expect(page.locator(".rua-chat-remote-pending")).toContainText(
        "Delivered, waiting for result",
      );
      await expect(page.locator(".rua-chat-message--user")).toHaveCount(1);
      await expect(page.locator(".rua-owned-composer")).toContainText(
        "Desktop",
      );
      suffix = "/console/submit";
    } else if (operation === "upload") {
      await page.locator('input[type="file"]').setInputFiles({
        name: "note.txt",
        mimeType: "text/plain",
        buffer: Buffer.from("hello"),
      });
      suffix = "/media";
    } else {
      await page.getByRole("button", { name: "More", exact: true }).click();
      await page
        .getByRole("menuitem", { name: "Enter ACP pass-through", exact: true })
        .click();
      await expect(page.locator(".rua-acp-enter-dialog")).toBeVisible();
      expect(created).toBe(false);
      await page
        .locator(".rua-acp-enter-dialog")
        .getByRole("button", { name: "New", exact: true })
        .click();
      suffix = "/acp/enter";
    }
    await expect
      .poll(() =>
        calls.some(
          (call) => call.path.endsWith(suffix) && call.method === "POST",
        ),
      )
      .toBe(true);
    expect(
      calls.filter((call) => call.path.endsWith("/materialize")),
    ).toHaveLength(1);
    expect(
      calls.filter((call) => call.path.endsWith("/authority-placement")),
    ).toHaveLength(0);
  });
}
