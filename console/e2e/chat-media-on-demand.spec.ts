import { createHash, randomUUID } from "node:crypto";
import { readFileSync, writeFileSync } from "node:fs";
import { expect, test } from "@playwright/test";
import { mockConsoleTransport } from "./fixtures/consoleTransport";

const png = Buffer.from(
  "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+/1gAAAABJRU5ErkJggg==",
  "base64",
);
const checksum = (bytes: Buffer) =>
  createHash("sha256").update(bytes).digest("hex");

test("a staged browser upload survives submission and a Rua service restart", async ({
  page,
  request,
}) => {
  const state = process.env.RUA_LIVE_CHAT_MEDIA_STATE;
  const phase = process.env.RUA_LIVE_CHAT_MEDIA_PHASE;
  test.skip(
    !state || !phase,
    "Requires the isolated Rua server and loopback model",
  );
  await page.addInitScript(() => localStorage.setItem("language", "en"));
  const reads: string[] = [];
  page.on("request", (req) => {
    if (
      req.method() === "GET" &&
      /\/media\/[^/]+$/.test(new URL(req.url()).pathname)
    )
      reads.push(new URL(req.url()).pathname);
  });
  if (phase === "submit") {
    await page.goto("/");
    const input = page.locator(".rua-owned-composer textarea");
    await expect(input).toBeVisible();
    const chatId = new URL(page.url()).pathname.split("/").at(-1);
    const base = "/api/chats/" + chatId;
    const uploaded = page.waitForResponse(
      (res) =>
        new URL(res.url()).pathname === base + "/media" &&
        res.request().method() === "POST",
    );
    await page.locator("input[type=file]").setInputFiles({
      name: "durable-image.png",
      mimeType: "image/png",
      buffer: png,
    });
    const upload = await uploaded;
    expect(upload.ok(), await upload.text()).toBe(true);
    const { media_id, media_ref } = await upload.json();
    expect(media_ref).toBe("media/" + media_id);
    expect((await request.get(base)).status()).toBe(404);
    expect(reads).toEqual([]);
    const stagedRead = page.waitForResponse(
      (res) => new URL(res.url()).pathname === base + "/media/" + media_id,
    );
    await page
      .getByRole("button", { name: "Open durable-image.png", exact: true })
      .click();
    const preview = await stagedRead;
    expect(preview.ok(), "preview of staged media").toBe(true);
    expect(checksum(await preview.body())).toBe(checksum(png));
    await expect(page.getByRole("dialog").locator("img")).toHaveAttribute(
      "src",
      /^blob:/,
    );
    await page.keyboard.press("Escape");
    expect((await request.get(base)).status()).toBe(404);
    await input.press("End");
    await input.pressSequentially(" Please describe this attachment.");
    await input.press("Enter");
    await expect(
      page.locator('[data-rua-chat-role="assistant"]').last(),
    ).toContainText("The shared Chat survived restart.", { timeout: 60000 });
    const history = await (await request.get(base)).json();
    expect(JSON.stringify(history.messages)).toContain(media_ref);
    writeFileSync(
      state!,
      JSON.stringify({ chatId, media_id, messages: history.messages }),
    );
    console.info(
      "Uploaded and submitted",
      chatId,
      media_id,
      "previews",
      reads.length,
    );
  } else {
    expect(phase).toBe("read");
    const { chatId, media_id, messages } = JSON.parse(
      readFileSync(state!, "utf8"),
    );
    const base = "/api/chats/" + chatId;
    const history = await (await request.get(base)).json();
    expect(history.messages).toEqual(messages);
    await page.goto("/chats/" + chatId);
    await expect(
      page.locator('[data-rua-chat-role="assistant"]').last(),
    ).toContainText("The shared Chat survived restart.");
    expect(reads).toEqual([]);
    const marker = page.locator('a[href="#rua-image-1"]').first();
    const image = page.waitForResponse(
      (res) => new URL(res.url()).pathname === base + "/media/" + media_id,
    );
    await marker.click();
    const response = await image;
    expect(response.ok()).toBe(true);
    expect(checksum(await response.body())).toBe(checksum(png));
    const dialogImage = page.getByRole("dialog").locator("img");
    await expect(dialogImage).toHaveAttribute("src", /^blob:/);
    await expect
      .poll(() =>
        dialogImage.evaluate((img: HTMLImageElement) => img.naturalWidth),
      )
      .toBe(1);
    await page.keyboard.press("Escape");
    await marker.click();
    await expect(dialogImage).toHaveAttribute("src", /^blob:/);
    expect(reads).toEqual([base + "/media/" + media_id]);
    await page.keyboard.press("Escape");
    // Chromium omits multipart file uploads from Network.getRequestPostData.
    // Observe the non-file fields and let the browser send the original body.
    const formFields: Array<{ epoch: string | null; agent: boolean }> = [];
    await page.exposeFunction(
      "captureUploadFields",
      (fields: (typeof formFields)[number]) => formFields.push(fields),
    );
    await page.evaluate(() => {
      const send = window.fetch;
      window.fetch = (url, init) => {
        if (init?.body instanceof FormData && init.body.has("file")) {
          const epoch = init.body.get("expected_authority_epoch");
          void (
            window as Window & {
              captureUploadFields(fields: {
                epoch: string | null;
                agent: boolean;
              }): Promise<void>;
            }
          ).captureUploadFields({
            epoch: typeof epoch === "string" ? epoch : null,
            agent: init.body.has("agent_id"),
          });
        }
        return send(url, init);
      };
    });
    const secondUpload = page.waitForResponse(
      (res) =>
        new URL(res.url()).pathname === base + "/media" &&
        res.request().method() === "POST",
    );
    await page.locator("input[type=file]").setInputFiles({
      name: "follow-up.txt",
      mimeType: "text/plain",
      buffer: Buffer.from("second upload"),
    });
    const saved = await secondUpload;
    expect(saved.ok(), await saved.text()).toBe(true);
    const { authority_epoch } = await saved.json();
    expect(authority_epoch).toBeTruthy();
    expect(formFields).toEqual([{ epoch: authority_epoch, agent: false }]);
    console.info(
      "Restarted Chat",
      chatId,
      "read",
      media_id,
      checksum(png),
      "once",
    );
  }
});

test("history and location load no objects, and only the selected Chat media is read or retried", async ({
  page,
}) => {
  await mockConsoleTransport(page);
  await page.addInitScript(() => {
    localStorage.setItem("language", "en");
    sessionStorage.setItem(
      "rua-agent-storage",
      JSON.stringify({ state: { selectedAgent: "other-agent" }, version: 0 }),
    );
  });
  const chat = randomUUID();
  const [image, audio, video, file, figure, poster] = Array.from(
    { length: 6 },
    () => randomUUID(),
  );
  const reference = (id: string) => "media/" + id;
  const base = "/api/chats/" + chat;
  const url = (id: string) => base + "/" + reference(id);
  const results: string[] = [];
  const unexpectedReads: string[] = [];
  await page.route("https://invalid.test/**", (route) => {
    unexpectedReads.push(route.request().url());
    return route.abort();
  });
  let available = false;
  await page.route(
    (u) => u.pathname.startsWith("/api/"),
    (route) => {
      const path = new URL(route.request().url()).pathname;
      const json = (data: unknown) => route.fulfill({ json: data });
      if (/\/media\/|\/files\/preview/.test(path)) {
        results.push(path);
        if (path === url(image) && !available)
          return route.fulfill({
            status: 503,
            json: { detail: "Source offline" },
          });
        return route.fulfill({ body: png, contentType: "image/png" });
      }
      if (path === "/api/agents")
        return json({
          agents: [
            { id: "default", name: "Default", enabled: true },
            { id: "other-agent", name: "Other", enabled: true },
          ],
        });
      if (path === "/api/projects") return json({ projects: [] });
      if (path === "/api/auth/status") return json({ enabled: false });
      if (path === "/api/sync/devices")
        return json({
          devices: [
            {
              device_id: "service",
              friendly_name: "Service desktop",
              is_self: true,
            },
          ],
        });
      if (path === base + "/agent-selection")
        return json({
          selection: { agent_id: "default", revision: 1 },
          switchable: true,
          blockers: [],
        });
      if (path === base)
        return json({
          id: chat,
          agent_id: "default",
          project_id: null,
          status: "idle",
          meta: {
            execution_directory: {
              device_id: "service",
              path: "/chat",
              managed: true,
            },
          },
          messages: [
            {
              id: "user",
              turn_id: "turn",
              role: "user",
              content: [
                { type: "text", text: "Inventory" },
                {
                  type: "image",
                  media_ref: reference(image),
                  filename: "sample.png",
                },
                {
                  type: "audio",
                  media_ref: reference(audio),
                  filename: "audio.wav",
                },
                {
                  type: "video",
                  media_ref: reference(video),
                  filename: "clip.mp4",
                  video_poster: reference(poster),
                },
                {
                  type: "image",
                  media_ref: "/raw/path.png",
                  image_url: "https://invalid.test/wrong.png",
                  filename: "wrong.png",
                },
              ],
            },
            {
              id: "assistant",
              turn_id: "turn",
              role: "assistant",
              type: "message",
              content: [
                {
                  type: "text",
                  text:
                    "Generated outputs\n\n![Markdown image](media/" +
                    image +
                    ")\n\n@figure\n  - src=media/" +
                    figure +
                    ' caption="Figure"\n@end',
                },
                {
                  type: "image",
                  media_ref: reference(figure),
                  filename: "result.png",
                },
                {
                  type: "file",
                  media_ref: reference(file),
                  filename: "report.txt",
                },
              ],
            },
          ],
        });
      if (path === "/api/chats" || /\/tasks$|\/models|\/channels/.test(path))
        return json([]);
      return json({});
    },
  );
  await page.goto("/chats/" + chat);
  await expect(page.getByRole("link", { name: "report.txt" })).toHaveAttribute(
    "href",
    url(file),
  );
  await expect(
    page.getByText("wrong.png · Attachment unavailable"),
  ).toBeVisible();
  const figureButton = page.getByRole("button", {
    name: "Open Figure",
    exact: true,
  });
  await figureButton.scrollIntoViewIfNeeded();
  const location = page
    .locator(".rua-owned-composer")
    .getByRole("button", { name: "Execution location", exact: true });
  await location.click();
  await expect(page.getByRole("dialog")).toBeVisible();
  await page.keyboard.press("Escape");
  expect(results).toEqual([]);
  await page
    .getByRole("button", { name: "Open Markdown image", exact: true })
    .click();
  await expect(page.getByRole("alert")).toContainText("Source offline");
  expect(results).toEqual([url(image)]);
  available = true;
  await page.getByRole("button", { name: "Retry", exact: true }).click();
  await expect(page.getByRole("dialog").locator("img")).toHaveAttribute(
    "src",
    /^blob:/,
  );
  await page.keyboard.press("Escape");
  await page.locator('a[href="#rua-image-1"]').first().click();
  await expect(page.getByRole("dialog").locator("img")).toHaveAttribute(
    "src",
    /^blob:/,
  );
  expect(results).toEqual([url(image), url(image)]);
  await page.keyboard.press("Escape");
  await figureButton.click();
  await expect(page.getByRole("dialog").locator("img")).toHaveAttribute(
    "src",
    /^blob:/,
  );
  await page.keyboard.press("Escape");
  await page
    .getByRole("button", { name: "Open result.png", exact: true })
    .click();
  await expect(page.getByRole("dialog").locator("img")).toHaveAttribute(
    "src",
    /^blob:/,
  );
  await page.keyboard.press("Escape");
  await page
    .getByRole("button", { name: "Play audio.wav", exact: true })
    .click();
  await expect(page.locator("audio")).toHaveAttribute("src", /^blob:/);
  await page
    .getByRole("button", { name: "Play clip.mp4", exact: true })
    .click();
  await expect(page.locator("video")).toHaveAttribute("src", /^blob:/);
  expect(unexpectedReads).toEqual([]);
  expect(results).toEqual([
    url(image),
    url(image),
    url(figure),
    url(audio),
    url(video),
  ]);
});
