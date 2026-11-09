import {
  test as base,
  expect,
  type Page,
  type Request,
} from "@playwright/test";

export const DEFAULT_PROMPT = "现在执行一个 sleep 155 秒然后 echo 15555 的命令";

export interface DiagnoseFixtures {
  backendReady: void;
  consoleLogSink: { messages: string[] };
}

export const test = base.extend<DiagnoseFixtures>({
  backendReady: [
    async ({ baseURL, request }, use) => {
      const target = `${baseURL ?? "http://127.0.0.1:8090"}/api/version`;
      const fallbacks = ["/api/agents", "/"];
      let reachable = false;
      try {
        const res = await request.get(target, { timeout: 5_000 });
        reachable = res.ok();
      } catch {
        reachable = false;
      }
      if (!reachable) {
        for (const path of fallbacks) {
          try {
            const res = await request.get(
              `${baseURL ?? "http://127.0.0.1:8090"}${path}`,
              { timeout: 5_000 },
            );
            if (res.status() < 500) {
              reachable = true;
              break;
            }
          } catch {}
        }
      }
      if (!reachable) {
        throw new Error(
          "[diagnose-e2e] backend not reachable at " +
            (baseURL ?? "http://127.0.0.1:8090") +
            ". Start it manually:\n" +
            "  cargo run -p rua-server -- --debug --no-auth \\\n" +
            "    --console-static-dir console/dist\n" +
            "and ensure a real LLM provider is configured.",
        );
      }
      await use();
    },
    { auto: true },
  ],
  consoleLogSink: async ({ page }, fixtureUse, testInfo) => {
    const sink = { messages: [] as string[] };
    page.on("console", (msg) => {
      const text = `[${msg.type()}] ${msg.text()}`;
      sink.messages.push(text);
    });
    await fixtureUse(sink);
    if (testInfo.status !== testInfo.expectedStatus) {
      const blob = sink.messages.join("\n");
      await testInfo.attach("browser-console.log", {
        body: blob,
        contentType: "text/plain",
      });
    }
  },
});

export { expect } from "@playwright/test";

export async function goChat(page: Page): Promise<void> {
  await page.goto("/", { waitUntil: "domcontentloaded" });

  await page
    .locator(".rua-sender textarea, textarea")
    .first()
    .waitFor({ state: "visible", timeout: 20_000 });
}

export async function navigateSidebar(
  page: Page,
  target: "mcp" | "chat" | "chats",
): Promise<void> {
  const candidates: Record<typeof target, RegExp[]> = {
    mcp: [/MCP/i],
    chat: [/chat/i, /对话/, /聊天/],
    chats: [/chats?/i, /会话/, /历史/],
  };
  for (const re of candidates[target]) {
    const link = page
      .locator(`a[href^="/${target}"], a:has-text("${re.source}")`)
      .first();
    if (await link.count()) {
      await link.click({ trial: false }).catch(() => {});
      return;
    }
  }

  await page.goto(target === "chat" ? "/" : `/${target}`);
}

export async function submitPrompt(page: Page, text: string): Promise<void> {
  const textarea = page.locator(".rua-sender textarea, textarea").first();
  await textarea.fill(text);

  const sendBtn = page.locator(
    'button[type="submit"], button:has-text("发送"), button:has-text("Send"), button.rua-sender-send',
  );
  if (
    await sendBtn
      .first()
      .isVisible()
      .catch(() => false)
  ) {
    await sendBtn
      .first()
      .click()
      .catch(async () => {
        await textarea.press("Enter");
      });
  } else {
    await textarea.press("Enter");
  }
}

export async function waitForChatUrl(
  page: Page,
  timeoutMs = 30_000,
): Promise<string> {
  await page.waitForURL((url) => /\/chat\/[^/]+/.test(url.pathname), {
    timeout: timeoutMs,
  });
  const m = page.url().match(/\/agents\/[^/?#]+\/chats\/([^/?#]+)/);
  if (!m) throw new Error("URL never became /agents/<agentId>/chats/<chatId>");
  return m[1];
}

export function captureChatStreamRequest(page: Page): Promise<Request> {
  return page.waitForRequest(
    (req) =>
      /\/api\/agents\/[^/]+\/console\/chat$/.test(req.url()) &&
      req.method() === "POST",
    { timeout: 30_000 },
  );
}

export function captureChatDeleteRequest(page: Page): Promise<Request> {
  return page.waitForRequest(
    (req) =>
      /\/api\/agents\/[^/]+\/chats\/[^/]+/.test(req.url()) &&
      req.method() === "DELETE",
    { timeout: 30_000 },
  );
}

export { expect as defaultExpect };
