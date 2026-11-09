import { test, expect, type Page } from "@playwright/test";

const OLD_CHAT_URL =
  process.env.RUA_OLD_CHAT_URL ??
  "http://127.0.0.1:5174/agents/default/chats/281d938b-98eb-46dc-88a1-b7e553bbd887";
const NEW_CHAT_URL =
  process.env.RUA_NEW_CHAT_URL ??
  "http://127.0.0.1:8765/agents/default/chats/281d938b-98eb-46dc-88a1-b7e553bbd887";

type Rect = {
  x: number;
  y: number;
  w: number;
  h: number;
};

type ChatMetrics = {
  title: string | null;
  placeholder: string | null;
  headerButtons: number;
  composerButtons: number;
  header: Rect | null;
  list: Rect | null;
  composerShell: Rect | null;
  composer: Rect | null;
  textarea: Rect | null;
  firstComposerButtons: Rect[];
  visibleAssistant: Rect | null;
  visibleAssistantMarkdown: Rect | null;
  visibleToolGroups: number;
};

type PixelDiffStats = {
  width: number;
  height: number;
  changed: number;
  changedRatio: number;
  avgChannelDeltaPerPixel: number;
  maxPixelDelta: number;
};

test.skip(
  process.env.RUA_CHAT_PARITY !== "1",
  "Set RUA_CHAT_PARITY=1 and run with the old/new chat URLs available.",
);

async function gotoStable(page: Page, url: string) {
  await page.addInitScript(() => {
    try {
      window.localStorage.setItem("rua-theme", "light");
      window.localStorage.setItem("rua-skin", "standard");
      window.localStorage.setItem("rua-color-scheme", "blue");
      window.localStorage.setItem("language", "zh");
    } catch {}
  });
  await page.addStyleTag({
    content: `
      :root {
        --app-bg-image: none !important;
        --app-bg-overlay: transparent !important;
      }
      *, *::before, *::after {
        animation-duration: 0s !important;
        animation-delay: 0s !important;
        transition-duration: 0s !important;
        scroll-behavior: auto !important;
        caret-color: transparent !important;
      }
    `,
  });
  await page.goto(url, { waitUntil: "domcontentloaded" });
  await page.evaluate(async () => {
    await document.fonts?.ready;
  });
  await page.waitForTimeout(1_000);
}

function sameRect(actual: Rect | null, expected: Rect | null) {
  expect(actual).toEqual(expected);
}

function sameNumberWithin(
  actual: number | undefined,
  expected: number | undefined,
  tolerance: number,
) {
  expect(actual).toBeDefined();
  expect(expected).toBeDefined();
  expect(Math.abs(actual! - expected!)).toBeLessThanOrEqual(tolerance);
}

async function collectMetrics(page: Page): Promise<ChatMetrics> {
  return page.evaluate(() => {
    function rect(selector: string): Rect | null {
      const el = document.querySelector(selector);
      if (!el) return null;
      const r = el.getBoundingClientRect();
      return {
        x: Math.round(r.x),
        y: Math.round(r.y),
        w: Math.round(r.width),
        h: Math.round(r.height),
      };
    }

    function rects(selector: string): Rect[] {
      return Array.from(document.querySelectorAll(selector)).map((el) => {
        const r = el.getBoundingClientRect();
        return {
          x: Math.round(r.x),
          y: Math.round(r.y),
          w: Math.round(r.width),
          h: Math.round(r.height),
        };
      });
    }

    function visible(selector: string): Rect | null {
      const items = Array.from(document.querySelectorAll(selector))
        .map((el) => {
          const r = el.getBoundingClientRect();
          return {
            x: Math.round(r.x),
            y: Math.round(r.y),
            w: Math.round(r.width),
            h: Math.round(r.height),
          };
        })
        .filter((item) => item.y < window.innerHeight && item.y + item.h > 0);
      return items[items.length - 1] ?? null;
    }

    const input = document.querySelector(
      ".rua-owned-composer textarea, .rua-sender textarea",
    );

    return {
      title:
        document
          .querySelector(".rua-owned-chat-title, [class*=breadcrumbCurrent]")
          ?.textContent?.trim() ?? null,
      placeholder: input?.getAttribute("placeholder") ?? null,
      headerButtons: document.querySelectorAll(
        ".rua-owned-chat-header-actions .rua-btn, .rua-chat-anywhere-layout-right-header .rua-btn",
      ).length,
      composerButtons: document.querySelectorAll(
        ".rua-owned-composer .rua-btn, .rua-sender .rua-btn",
      ).length,
      header: rect(
        ".rua-owned-chat-header, .rua-chat-anywhere-layout-right-header",
      ),
      list: rect(".rua-owned-surface-messages, .rua-bubble-list-wrapper"),
      composerShell: rect(
        ".rua-owned-surface-composer, .rua-chat-anywhere-input",
      ),
      composer: rect(".rua-owned-composer, .rua-sender"),
      textarea: rect(".rua-owned-composer textarea, .rua-sender textarea"),
      firstComposerButtons: rects(
        ".rua-owned-composer .rua-btn, .rua-sender .rua-btn",
      ).slice(0, 8),
      visibleAssistant: visible(
        '[data-role="assistant"], [data-rua-chat-role="assistant"]',
      ),
      visibleAssistantMarkdown: visible(
        ".rua-local-markdown, .rua-chat-markdown",
      ),
      visibleToolGroups: rects(".rua-tool-group").filter(
        (item) => item.y < window.innerHeight && item.y + item.h > 0,
      ).length,
    };
  });
}

async function compareScreenshots(
  page: Page,
  oldPng: Buffer,
  newPng: Buffer,
): Promise<PixelDiffStats> {
  return page.evaluate(
    async ({ oldB64, newB64 }) => {
      async function loadPng(b64: string): Promise<ImageBitmap> {
        const response = await fetch(`data:image/png;base64,${b64}`);
        return createImageBitmap(await response.blob());
      }

      const [oldImage, newImage] = await Promise.all([
        loadPng(oldB64),
        loadPng(newB64),
      ]);
      const width = Math.min(oldImage.width, newImage.width);
      const height = Math.min(oldImage.height, newImage.height);
      const canvas = new OffscreenCanvas(width, height);
      const context = canvas.getContext("2d", { willReadFrequently: true });
      if (!context) throw new Error("Unable to create image diff context");

      context.drawImage(oldImage, 0, 0);
      const oldData = context.getImageData(0, 0, width, height).data;
      context.clearRect(0, 0, width, height);
      context.drawImage(newImage, 0, 0);
      const newData = context.getImageData(0, 0, width, height).data;

      let changed = 0;
      let totalDelta = 0;
      let maxPixelDelta = 0;
      for (let index = 0; index < oldData.length; index += 4) {
        const pixelDelta =
          Math.abs(oldData[index] - newData[index]) +
          Math.abs(oldData[index + 1] - newData[index + 1]) +
          Math.abs(oldData[index + 2] - newData[index + 2]) +
          Math.abs(oldData[index + 3] - newData[index + 3]);
        if (pixelDelta > 16) changed += 1;
        totalDelta += pixelDelta;
        maxPixelDelta = Math.max(maxPixelDelta, pixelDelta);
      }

      const pixels = width * height;
      return {
        width,
        height,
        changed,
        changedRatio: changed / pixels,
        avgChannelDeltaPerPixel: totalDelta / (pixels * 4),
        maxPixelDelta,
      };
    },
    {
      oldB64: oldPng.toString("base64"),
      newB64: newPng.toString("base64"),
    },
  );
}

test("owned chat shell matches the legacy chat shell geometry", async ({
  browser,
}, testInfo) => {
  const context = await browser.newContext({
    viewport: { width: 1440, height: 1000 },
    deviceScaleFactor: 1,
  });
  const oldPage = await context.newPage();
  const newPage = await context.newPage();

  await gotoStable(oldPage, OLD_CHAT_URL);
  await gotoStable(newPage, NEW_CHAT_URL);

  const [oldPng, newPng] = await Promise.all([
    oldPage.screenshot({ fullPage: false }),
    newPage.screenshot({ fullPage: false }),
  ]);
  await testInfo.attach("legacy-chat.png", {
    body: oldPng,
    contentType: "image/png",
  });
  await testInfo.attach("owned-chat.png", {
    body: newPng,
    contentType: "image/png",
  });

  const oldMetrics = await collectMetrics(oldPage);
  const newMetrics = await collectMetrics(newPage);
  await testInfo.attach("chat-parity-metrics.json", {
    body: JSON.stringify({ old: oldMetrics, new: newMetrics }, null, 2),
    contentType: "application/json",
  });
  const diffPage = await context.newPage();
  const pixelDiff = await compareScreenshots(diffPage, oldPng, newPng);
  await diffPage.close();
  await testInfo.attach("chat-parity-pixel-diff.json", {
    body: JSON.stringify(pixelDiff, null, 2),
    contentType: "application/json",
  });

  expect(newMetrics.title).toBe(oldMetrics.title);
  expect(newMetrics.placeholder).toBe(oldMetrics.placeholder);
  expect(newMetrics.headerButtons).toBe(oldMetrics.headerButtons);
  expect(newMetrics.composerButtons).toBe(oldMetrics.composerButtons);

  sameRect(newMetrics.header, oldMetrics.header);
  sameRect(newMetrics.composerShell, oldMetrics.composerShell);
  sameRect(newMetrics.composer, oldMetrics.composer);
  sameRect(newMetrics.textarea, oldMetrics.textarea);
  expect(newMetrics.firstComposerButtons).toEqual(
    oldMetrics.firstComposerButtons,
  );

  expect(newMetrics.visibleAssistant?.x).toBe(oldMetrics.visibleAssistant?.x);
  sameNumberWithin(
    newMetrics.visibleAssistant?.y,
    oldMetrics.visibleAssistant?.y,
    4,
  );
  expect(newMetrics.visibleAssistant?.w).toBe(oldMetrics.visibleAssistant?.w);
  expect(newMetrics.visibleAssistant?.h).toBe(oldMetrics.visibleAssistant?.h);
  expect(newMetrics.visibleAssistantMarkdown?.x).toBe(
    oldMetrics.visibleAssistantMarkdown?.x,
  );
  sameNumberWithin(
    newMetrics.visibleAssistantMarkdown?.y,
    oldMetrics.visibleAssistantMarkdown?.y,
    4,
  );
  expect(newMetrics.visibleAssistantMarkdown?.w).toBe(
    oldMetrics.visibleAssistantMarkdown?.w,
  );
  expect(newMetrics.visibleAssistantMarkdown?.h).toBe(
    oldMetrics.visibleAssistantMarkdown?.h,
  );
  expect(newMetrics.visibleToolGroups).toBe(oldMetrics.visibleToolGroups);
  expect(pixelDiff.changedRatio).toBeLessThanOrEqual(0.006);
  expect(pixelDiff.avgChannelDeltaPerPixel).toBeLessThanOrEqual(0.35);

  await context.close();
});
