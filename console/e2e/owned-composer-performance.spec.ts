import { expect, test } from "@playwright/test";

type ComposerScenario = typeof window & {
  composerStream: (payload: unknown) => void;
  switchComposerChat: (id: string) => void;
  storageReads: number;
  storageWrites: number;
  historyUpdates: number;
  historyUpdatesBeforeTyping: number;
  historyMutations: number;
  textareaHeightReads: number;
};

// Exercise the actual surface, composer and runtime against synthetic data.
// All application requests are handled here, without a server or LLM provider.
test.beforeEach(async ({ page }, testInfo) => {
  await page.addInitScript(() => {
    const originalFetch = window.fetch.bind(window);
    window.fetch = (input, init) => {
      const url =
        typeof input === "string"
          ? input
          : input instanceof URL
          ? input.href
          : input.url;
      if (url.includes("/console/live")) {
        return Promise.resolve(
          new Response(
            new ReadableStream({
              start(controller) {
                const encoder = new TextEncoder();
                (window as ComposerScenario).composerStream = (
                  payload: unknown,
                ) =>
                  controller.enqueue(
                    encoder.encode(`data: ${JSON.stringify(payload)}\n\n`),
                  );
              },
            }),
            { headers: { "Content-Type": "text/event-stream" } },
          ),
        );
      }
      return originalFetch(input, init);
    };
  });
  await page.route(
    (url) => url.pathname.startsWith("/api/"),
    async (route) => {
      const url = new URL(route.request().url());
      const messages = Array.from({ length: 200 }, (_, index) => [
        {
          id: `user-${index}`,
          turn_id: `turn-${index}`,
          role: "user",
          content: `Question ${index}`,
        },
        {
          id: `answer-${index}`,
          turn_id: `turn-${index}`,
          role: "assistant",
          type: "message",
          content: `## Answer ${index}\n\nA **formatted** answer.\n\n- First point\n- Second point`,
        },
      ]).flat();
      let body: unknown = {};
      if (/\/chats\/[^/]+$/.test(url.pathname)) {
        body = {
          title: "Composer test",
          status: "idle",
          model: { provider_id: "test", model: "test" },
          messages,
          ...(testInfo.title.includes("collaboration surface")
            ? {
                meta: {
                  collaboration: {
                    source: {
                      scope: { kind: "agent", agent_id: "composer-agent" },
                      chat_id: "source",
                    },
                    executor: {
                      kind: "acp",
                      runner: "long-runner-name-for-layout-check",
                    },
                    executor_agent_id: "composer-agent",
                    source_available: true,
                    configuration: {
                      status: "ready",
                      applied_source_epoch: "private-epoch",
                      desired_source_epoch: "private-epoch",
                    },
                  },
                },
              }
            : {}),
        };
      } else if (url.pathname.endsWith("/commands")) {
        body = [];
      } else if (
        url.pathname.includes("approval") ||
        url.pathname.includes("permission")
      ) {
        body = { pending: [], resolved: [] };
      } else if (url.pathname.includes("active-models")) {
        body = { active_model: { provider_id: "test", model: "test" } };
      }
      await route.fulfill({ json: body });
    },
  );
  await page.goto("/e2e/fixtures/owned-composer.html");
  await expect(page.locator(".rua-chat-message")).toHaveCount(400);
});

for (const width of [390, 1440]) {
  test(`collaboration surface keeps compact header and usable messages at ${width}px`, async ({
    page,
  }, testInfo) => {
    await page.setViewportSize({ width, height: 844 });
    const relation = page.locator(".rua-owned-collaboration-relationship");
    await expect(relation).toBeVisible();
    await expect(relation).not.toContainText("private-epoch");
    const header = (await relation.boundingBox())!;
    expect(header.height).toBeLessThan(100);
    const messages = (await page
      .locator(".rua-owned-surface-messages")
      .boundingBox())!;
    expect(messages.height).toBeGreaterThan(300);
    expect(messages.y).toBeGreaterThanOrEqual(header.y + header.height - 1);
    const input = page.locator(".rua-owned-composer textarea");
    await input.fill("Continue the scoped work");
    await expect(input).toHaveValue("Continue the scoped work");
    await expect(relation.getByRole("button")).toBeEnabled();
    await page.screenshot({
      path: testInfo.outputPath("collaboration-surface.png"),
    });
  });
}

test("typing with long history avoids stored-draft reads and history recalculation", async ({
  page,
}, testInfo) => {
  const input = page.locator(".rua-owned-composer textarea");
  await page.evaluate(() => {
    const state = window as ComposerScenario;
    state.storageReads = 0;
    state.storageWrites = 0;
    state.textareaHeightReads = 0;
    const input = document.querySelector(".rua-owned-composer textarea")!;
    const readHeight = Object.getOwnPropertyDescriptor(
      Element.prototype,
      "scrollHeight",
    )!.get!;
    Object.defineProperty(input, "scrollHeight", {
      get() {
        state.textareaHeightReads += 1;
        return readHeight.call(this);
      },
    });
    state.historyUpdatesBeforeTyping = state.historyUpdates;
    const get = Storage.prototype.getItem;
    const set = Storage.prototype.setItem;
    Storage.prototype.getItem = function (key) {
      if (key.startsWith("rua_input_draft_")) state.storageReads += 1;
      return get.call(this, key);
    };
    Storage.prototype.setItem = function (key, value) {
      if (key.startsWith("rua_input_draft_")) state.storageWrites += 1;
      return set.call(this, key, value);
    };
    const oldMessage = document.querySelector(".rua-chat-message-list")!;
    state.historyMutations = 0;
    new MutationObserver(
      (records) =>
        (state.historyMutations += records.filter(
          (record) =>
            record.type === "childList" || record.type === "characterData",
        ).length),
    ).observe(oldMessage, {
      subtree: true,
      childList: true,
      characterData: true,
    });
  });
  await input.pressSequentially("Testing a normal draft with history.", {
    delay: 5,
  });
  await expect(input).toHaveValue("Testing a normal draft with history.");
  await expect(input).not.toHaveClass(/has-marker-layer/);
  const metrics = await page.evaluate(() => {
    const state = window as ComposerScenario;
    return {
      reads: state.storageReads,
      writes: state.storageWrites,
      historyUpdates: state.historyUpdates - state.historyUpdatesBeforeTyping,
      historyMutations: state.historyMutations,
      nativeFieldSizing: CSS.supports("field-sizing", "content"),
      textareaHeightReads: state.textareaHeightReads,
      mountedMessages: document.querySelectorAll(".rua-chat-message").length,
    };
  });
  await testInfo.attach("composer-counters", {
    body: JSON.stringify(metrics, null, 2),
    contentType: "application/json",
  });
  expect(metrics.reads).toBe(0);
  expect(metrics.writes).toBeLessThan(10);
  expect(metrics.historyUpdates).toBe(0);
  expect(metrics.historyMutations).toBe(0);
  if (metrics.nativeFieldSizing) expect(metrics.textareaHeightReads).toBe(0);
});

test("textarea grows, shrinks and preserves native composition at narrow widths", async ({
  page,
}) => {
  const input = page.locator(".rua-owned-composer textarea");
  const initial = (await input.boundingBox())!.height;
  await input.fill(
    "A long line that should wrap naturally on a mobile-sized display. ".repeat(
      30,
    ),
  );
  await expect
    .poll(async () => (await input.boundingBox())!.height)
    .toBeGreaterThan(initial);
  expect((await input.boundingBox())!.height).toBeLessThanOrEqual(221);
  await page.locator("#rua-input-expand-btn").click();
  await expect
    .poll(async () => (await input.boundingBox())!.height)
    .toBeGreaterThan(220);
  expect((await input.boundingBox())!.height).toBeLessThanOrEqual(441);
  await input.fill("short");
  await page.locator("#rua-input-expand-btn").click();
  await expect
    .poll(async () => (await input.boundingBox())!.height)
    .toBeLessThanOrEqual(initial + 1);
  await input.dispatchEvent("compositionstart", { data: "" });
  await input.fill("ni");
  await expect(input).not.toHaveClass(/has-marker-layer/);
  await input.fill("你好");
  await input.dispatchEvent("compositionend", { data: "你好" });
  await expect(input).toHaveValue("你好");
  await page.setViewportSize({ width: 360, height: 480 });
  await expect(input).toBeVisible();
  await expect(input).toHaveValue("你好");
});

test("draft survives route switching and is flushed on backgrounding", async ({
  page,
}) => {
  const input = page.locator(".rua-owned-composer textarea");
  await input.fill("Unsaved Chinese draft 草稿");
  await page.evaluate(() =>
    (window as ComposerScenario).switchComposerChat("another-chat"),
  );
  await expect(input).toHaveValue("");
  await page.evaluate(() =>
    (window as ComposerScenario).switchComposerChat("composer-chat"),
  );
  await expect(input).toHaveValue("Unsaved Chinese draft 草稿");
  await input.fill("Updated 草稿");
  await page.evaluate(() =>
    window.dispatchEvent(new Event("rua:background")),
  );
  expect(
    await page.evaluate(() =>
      sessionStorage.getItem("rua_input_draft_composer-agent:composer-chat"),
    ),
  ).toBe("Updated 草稿");
});

test("streaming a reply preserves composition text and settled history", async ({
  page,
}) => {
  const input = page.locator(".rua-owned-composer textarea");
  const firstAnswer = page
    .locator('[data-rua-chat-role="assistant"]')
    .first();
  const originalAnswer = await firstAnswer.textContent();
  await input.dispatchEvent("compositionstart", { data: "" });
  await input.fill("zhong");
  await page.evaluate(() => {
    const state = window as ComposerScenario;
    state.composerStream({
      type: "user_message",
      message: {
        id: "live-user",
        turn_id: "live-turn",
        role: "user",
        content: "Continue",
      },
    });
    state.composerStream({
      type: "text_delta",
      message_id: "live-answer",
      delta: "Live reply",
    });
  });
  await expect(
    page.locator('[data-rua-chat-role="assistant"]').last(),
  ).toContainText("Live reply");
  await expect(input).toHaveValue("zhong");
  await expect(input).toBeFocused();
  await expect(input).not.toHaveClass(/has-marker-layer/);
  await input.fill("中文");
  await page.evaluate(() =>
    (window as ComposerScenario).composerStream({
      type: "text_delta",
      message_id: "live-answer",
      delta: " continues",
    }),
  );
  await expect(
    page.locator('[data-rua-chat-role="assistant"]').last(),
  ).toContainText("Live reply continues");
  await input.dispatchEvent("compositionend", { data: "中文" });
  await expect(input).toHaveValue("中文");
  await expect(firstAnswer).toHaveText(originalAnswer ?? "");
});

test("typing during streaming keeps a paused history anchor stationary", async ({
  page,
}) => {
  const input = page.locator(".rua-owned-composer textarea");
  const scroller = page.locator(".rua-owned-surface-messages");
  await scroller.evaluate((node) => {
    node.scrollTop = node.scrollHeight / 2;
  });
  await input.click();
  await page.waitForTimeout(250);
  await page.evaluate(() => {
    const node = document.querySelector(".rua-owned-surface-messages")!;
    const top = node.getBoundingClientRect().top;
    const anchor = [...node.querySelectorAll(".rua-chat-message")].find(
      (message) => {
        const rect = message.getBoundingClientRect();
        return rect.top >= top && rect.bottom <= top + node.clientHeight;
      },
    )!;
    const initialTop = anchor.getBoundingClientRect().top;
    const state = window as ComposerScenario & {
      stopAnchorProbe: () => number;
    };
    let maxShift = 0;
    let frame = 0;
    const sample = () => {
      maxShift = Math.max(
        maxShift,
        Math.abs(anchor.getBoundingClientRect().top - initialTop),
      );
      frame = requestAnimationFrame(sample);
    };
    frame = requestAnimationFrame(sample);
    state.composerStream({
      type: "user_message",
      message: {
        id: "live-user",
        turn_id: "live-turn",
        role: "user",
        content: "Continue",
      },
    });
    const stream = setInterval(
      () =>
        state.composerStream({
          type: "text_delta",
          message_id: "live-answer",
          delta: " more output",
        }),
      30,
    );
    state.stopAnchorProbe = () => {
      clearInterval(stream);
      cancelAnimationFrame(frame);
      return maxShift;
    };
  });
  await input.pressSequentially("typing while streaming", { delay: 40 });
  await expect(input).toHaveValue("typing while streaming");
  await expect(
    page.locator('[data-rua-chat-role="assistant"]').last(),
  ).toContainText("more output");
  const maxShift = await page.evaluate(() =>
    (
      window as typeof window & {
        stopAnchorProbe: () => number;
      }
    ).stopAnchorProbe(),
  );
  expect(maxShift).toBeLessThanOrEqual(1);
});

for (const sections of [160, 800, "code"] as const) {
  test(`long local Markdown (${sections} sections) keeps updating under CPU pressure`, async ({
    page,
    browserName,
  }) => {
    test.skip(browserName !== "chromium", "CPU throttling uses Chromium CDP");
    const cdp = await page.context().newCDPSession(page);
    await cdp.send("Emulation.setCPUThrottlingRate", { rate: 4 });
    try {
      await page.evaluate((sections) => {
        const state = window as ComposerScenario & {
          progress: {
            sent: number;
            rendered: number;
            maxLag: number;
            done: boolean;
          };
        };
        const progress = {
          sent: 0,
          rendered: 0,
          maxLag: 0,
          done: false,
          maxDisplayGap: 0,
          initialDisplayDelay: 0,
          lastDisplay: performance.now(),
        };
        state.progress = progress;
        state.composerStream({
          type: "user_message",
          message: {
            id: "pressure-user",
            turn_id: "pressure-turn",
            role: "user",
            content: "Long report",
          },
        });
        state.composerStream({
          type: "text_delta",
          message_id: "pressure-answer",
          delta:
            sections === "code"
              ? "```typescript\n" +
                Array.from(
                  { length: 300 },
                  (_, i) =>
                    `const value${i} = { id: ${i}, label: 'entry-${i}', enabled: true };`,
                ).join("\n") +
                "\n```\n\nTrailing explanation:"
              : Array.from(
                  { length: sections },
                  (_, i) =>
                    `\n\n### Section ${i}\n\nParagraph with **bold text**, a [link](https://example.com), and inline code \`const x = 1\`.\n\n- first point\n- second point`,
                ).join(""),
        });
        const timer = setInterval(() => {
          const text =
            document.querySelector(".rua-owned-surface-messages")
              ?.textContent ?? "";
          const matches = [...text.matchAll(/pressure-token-(\d+)-end/g)];
          const rendered = matches.length ? Number(matches.at(-1)![1]) + 1 : 0;
          if (rendered !== progress.rendered) {
            const now = performance.now();
            if (progress.rendered === 0)
              progress.initialDisplayDelay = now - progress.lastDisplay;
            else
              progress.maxDisplayGap = Math.max(
                progress.maxDisplayGap,
                now - progress.lastDisplay,
              );
            progress.lastDisplay = now;
          }
          progress.rendered = rendered;
          progress.maxLag = Math.max(
            progress.maxLag,
            progress.sent - progress.rendered,
          );
          if (progress.sent < 80) {
            state.composerStream({
              type: "text_delta",
              message_id: "pressure-answer",
              delta: ` pressure-token-${progress.sent}-end`,
            });
            progress.sent++;
          } else {
            clearInterval(timer);
            progress.done = true;
          }
        }, 40);
      }, sections);
      await expect
        .poll(
          () =>
            page.evaluate(
              () =>
                (window as unknown as { progress: { done: boolean } }).progress
                  .done,
            ),
          { timeout: 30000 },
        )
        .toBe(true);
      const progress = await page.evaluate(
        () =>
          (
            window as unknown as {
              progress: { sent: number; rendered: number; maxLag: number };
            }
          ).progress,
      );
      console.log("CPU constrained streaming", progress);
      expect(progress.maxLag).toBeLessThan(15);
      await expect(
        page.locator('[data-rua-chat-role="assistant"]').last(),
      ).toContainText("pressure-token-79-end");
    } finally {
      await cdp.send("Emulation.setCPUThrottlingRate", { rate: 1 });
      await cdp.detach();
    }
  });
}

test("burst text deltas are displayed promptly without waiting for completion", async ({
  page,
}) => {
  await page.evaluate(() => {
    (window as ComposerScenario).composerStream({
      type: "user_message",
      message: {
        id: "burst-user",
        turn_id: "burst-turn",
        role: "user",
        content: "Burst reply",
      },
    });
  });
  const output = page.locator('[data-rua-chat-role="assistant"]').last();
  for (let index = 0; index < 3; index++) {
    const text = `Chunk ${index}: ${"Received text. ".repeat(80)} end-${index}`;
    const started = Date.now();
    await page.evaluate(
      (delta) =>
        (window as ComposerScenario).composerStream({
          type: "text_delta",
          message_id: "burst-answer",
          delta,
        }),
      text,
    );
    await expect(output).toContainText(`end-${index}`, { timeout: 1500 });
    console.log("burst chunk display", {
      chunk: index,
      characters: text.length,
      observedMs: Date.now() - started,
    });
    // No completion event is sent. Each complete chunk must already be visible.
    await page.waitForTimeout(350);
  }
});

test("local steady Markdown deltas keep painting throughout generation", async ({
  page,
}) => {
  await page.evaluate(() => {
    const state = window as ComposerScenario & {
      streamProbe: {
        sent: number;
        paints: number[];
        started: number;
        done: boolean;
      };
    };
    const probe = {
      sent: 0,
      paints: [] as number[],
      started: performance.now(),
      done: false,
    };
    state.streamProbe = probe;
    state.composerStream({
      type: "user_message",
      message: {
        id: "steady-user",
        turn_id: "steady-turn",
        role: "user",
        content: "Generate a report",
      },
    });
    const container = document.querySelector(".rua-owned-surface-messages")!;
    const observer = new MutationObserver(() => {
      const text = container.textContent;
      if (text?.includes("steady-token")) probe.paints.push(performance.now());
    });
    observer.observe(container, {
      subtree: true,
      childList: true,
      characterData: true,
    });
    const timer = setInterval(() => {
      state.composerStream({
        type: "text_delta",
        message_id: "steady-answer",
        delta: `\n\n### Section ${probe.sent}\n\nsteady-token-${probe.sent}: **measured text** with [reference](https://example.com) and inline code \`value\`.\n\n- entry one\n- entry two\n`,
      });
      if (++probe.sent === 80) {
        clearInterval(timer);
        setTimeout(() => {
          observer.disconnect();
          probe.done = true;
        }, 250);
      }
    }, 40);
  });
  await expect
    .poll(
      () =>
        page.evaluate(
          () =>
            (window as unknown as { streamProbe: { done: boolean } })
              .streamProbe.done,
        ),
      { timeout: 15000 },
    )
    .toBe(true);
  const metrics = await page.evaluate(() => {
    const probe = (
      window as unknown as {
        streamProbe: { paints: number[]; started: number };
      }
    ).streamProbe;
    const times = [probe.started, ...probe.paints];
    return {
      updates: probe.paints.length,
      maxGap: Math.max(
        ...times.slice(1).map((time, index) => time - times[index]),
      ),
    };
  });
  console.log("steady stream rendering", metrics);
  expect(metrics.updates).toBeGreaterThan(20);
  expect(metrics.maxGap).toBeLessThan(600);
  await expect(
    page.locator('[data-rua-chat-role="assistant"]').last(),
  ).toContainText("steady-token-79");
});

for (const scenario of ["local", "self", "immediate", "delayed"] as const) {
  test(`${scenario} history hydration permits incremental display`, async ({
    page,
  }) => {
    if (scenario === "self") {
      await page.goto("/e2e/fixtures/owned-composer.html?device=desktop");
      await expect(page.locator(".rua-chat-message")).toHaveCount(400);
    }
    let release!: () => void;
    const gate = new Promise<void>((resolve) => {
      release = resolve;
    });
    let requested = false;
    await page.route("**/api/sync/cycle/target", async (route) => {
      requested = true;
      if (scenario === "delayed") await gate;
      await route.fulfill({ json: {} });
    });
    try {
      await page.evaluate(() =>
        (window as ComposerScenario).composerStream({
          type: "user_message",
          message: {
            id: "hydration-user",
            turn_id: "hydration-turn",
            role: "user",
            content: "Continue",
          },
        }),
      );
      if (scenario !== "local")
        await page.evaluate(() =>
          (window as ComposerScenario).composerStream({
            type: "stream_replay",
            complete: true,
            events: [
              {
                type: "history_watermark_reached",
                agent_id: "composer-agent",
                chat_id: "composer-chat",
                authority_device_id: "desktop",
                durable_sequence: 500,
                manifest_hash: "pending-history",
              },
            ],
          }),
        );
      if (scenario !== "local" && scenario !== "self")
        await expect.poll(() => requested).toBe(true);
      for (let index = 0; index < 4; index++) {
        await page.evaluate(
          (index) =>
            (window as ComposerScenario).composerStream({
              type: "text_delta",
              message_id: "live-hydration-answer",
              delta: `Visible chunk ${index} `,
            }),
          index,
        );
        await expect(
          page.locator('[data-rua-chat-role="assistant"]').last(),
        ).toContainText(`Visible chunk ${index}`, { timeout: 1500 });
      }
      if (scenario === "self") expect(requested).toBe(false);
    } finally {
      release();
    }
  });
}

test("small upward wheel movement releases bottom following without snapping back", async ({
  page,
}) => {
  const scroller = page.locator(".rua-owned-surface-messages");
  await expect
    .poll(() =>
      scroller.evaluate(
        (node) => node.scrollHeight - node.scrollTop - node.clientHeight,
      ),
    )
    .toBeLessThanOrEqual(1);
  await scroller.hover();
  await page.mouse.wheel(0, -4);
  await page.waitForTimeout(250);
  expect(
    await scroller.evaluate(
      (node) => node.scrollHeight - node.scrollTop - node.clientHeight,
    ),
  ).toBeGreaterThanOrEqual(3);
  await page.setViewportSize({ width: 390, height: 840 });
  await page.waitForTimeout(150);
  expect(
    await scroller.evaluate(
      (node) => node.scrollHeight - node.scrollTop - node.clientHeight,
    ),
  ).toBeGreaterThanOrEqual(3);
  await page.mouse.wheel(0, -40);
  await page.waitForTimeout(250);
  expect(
    await scroller.evaluate(
      (node) => node.scrollHeight - node.scrollTop - node.clientHeight,
    ),
  ).toBeGreaterThanOrEqual(40);
  await page.mouse.wheel(0, 200);
  await expect
    .poll(() =>
      scroller.evaluate(
        (node) => node.scrollHeight - node.scrollTop - node.clientHeight,
      ),
    )
    .toBeLessThanOrEqual(1);
  await page.setViewportSize({ width: 390, height: 836 });
  await expect
    .poll(() =>
      scroller.evaluate(
        (node) => node.scrollHeight - node.scrollTop - node.clientHeight,
      ),
    )
    .toBeLessThanOrEqual(1);
});

test("touch departure from bottom keeps its reading position across viewport resize", async ({
  page,
}) => {
  const scroller = page.locator(".rua-owned-surface-messages");
  await scroller.evaluate((node) => {
    node.dispatchEvent(new Event("touchmove", { bubbles: true }));
    node.scrollTop -= 4;
    node.dispatchEvent(new Event("scroll"));
    node.dispatchEvent(new Event("touchend", { bubbles: true }));
  });
  await page.setViewportSize({ width: 390, height: 840 });
  await page.waitForTimeout(200);
  expect(
    await scroller.evaluate(
      (node) => node.scrollHeight - node.scrollTop - node.clientHeight,
    ),
  ).toBeGreaterThanOrEqual(3);
});

test("typing within a grown textarea does not transiently resize the history viewport", async ({
  page,
}) => {
  await page.addInitScript(() => {
    const supports = CSS.supports.bind(CSS);
    CSS.supports = ((...args: string[]) =>
      args[0] === "field-sizing"
        ? false
        : supports(...(args as [string, string]))) as typeof CSS.supports;
  });
  await page.reload();
  const input = page.locator(".rua-owned-composer textarea");
  await expect(input).toBeVisible();
  await input.fill("Line one\nLine two\nLine three\nLine four\nshort");
  await input.evaluate((element) =>
    element.style.setProperty("field-sizing", "fixed"),
  );
  await input.pressSequentially(" ");
  await page.waitForTimeout(200);
  await page.evaluate(() => {
    const input = document.querySelector<HTMLTextAreaElement>(
      ".rua-owned-composer textarea",
    )!;
    const viewport = document.querySelector<HTMLElement>(
      ".rua-owned-surface-messages",
    )!;
    const original = Object.getOwnPropertyDescriptor(
      Element.prototype,
      "scrollHeight",
    )!.get!;
    const state = window as typeof window & {
      typingViewportHeights: number[];
      stableViewportHeight: number;
    };
    state.typingViewportHeights = [];
    state.stableViewportHeight = viewport.getBoundingClientRect().height;
    Object.defineProperty(Element.prototype, "scrollHeight", {
      configurable: true,
      get() {
        if (this instanceof HTMLTextAreaElement)
          state.typingViewportHeights.push(
            viewport.getBoundingClientRect().height,
          );
        return original.call(this);
      },
    });
  });
  await input.pressSequentially("abcdefgh", { delay: 30 });
  const metrics = await page.evaluate(() => {
    const state = window as typeof window & {
      typingViewportHeights: number[];
      stableViewportHeight: number;
    };
    return {
      heights: state.typingViewportHeights,
      baseline: state.stableViewportHeight,
    };
  });
  expect(metrics.heights.length).toBeGreaterThan(0);
  for (const height of metrics.heights)
    expect(Math.abs(height - metrics.baseline)).toBeLessThanOrEqual(1);
});

test("older engines keep the textarea height fallback", async ({ page }) => {
  await page.addInitScript(() => {
    const supports = CSS.supports.bind(CSS);
    CSS.supports = ((property: string, value?: string) => {
      if (property === "field-sizing") return false;
      return value === undefined
        ? supports(property)
        : supports(property, value);
    }) as typeof CSS.supports;
  });
  await page.reload();
  const input = page.locator(".rua-owned-composer textarea");
  await expect(input).toBeVisible();
  const initial = (await input.boundingBox())!.height;
  await input.fill("Long fallback text that wraps. ".repeat(80));
  await expect
    .poll(async () => (await input.boundingBox())!.height)
    .toBeGreaterThan(initial);
  expect((await input.boundingBox())!.height).toBeLessThanOrEqual(221);
  await input.fill("short");
  await expect
    .poll(async () => (await input.boundingBox())!.height)
    .toBeLessThanOrEqual(initial + 1);
});

test("keyboard-sized viewport changes do not remeasure every history message", async ({
  page,
}, testInfo) => {
  await page.evaluate(() => {
    const state = window as typeof window & { historicalHeightReads: number };
    state.historicalHeightReads = 0;
    const height = Object.getOwnPropertyDescriptor(
      Element.prototype,
      "scrollHeight",
    )!.get!;
    document
      .querySelectorAll(".rua-chat-user-content-frame")
      .forEach((element) => {
        Object.defineProperty(element, "scrollHeight", {
          get() {
            state.historicalHeightReads += 1;
            return height.call(this);
          },
        });
      });
  });
  await page.setViewportSize({ width: 390, height: 540 });
  await page.evaluate(
    () =>
      new Promise<void>((resolve) =>
        requestAnimationFrame(() => requestAnimationFrame(() => resolve())),
      ),
  );
  const reads = await page.evaluate(
    () =>
      (window as typeof window & { historicalHeightReads: number })
        .historicalHeightReads,
  );
  await testInfo.attach("history-height-measurements", {
    body: JSON.stringify({ messages: 400, reads }),
    contentType: "application/json",
  });
  expect(reads).toBe(0);
  await expect(page.locator(".rua-chat-message")).toHaveCount(400);
});

test("user-message collapsing still updates after width changes", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1100, height: 844 });
  await page.route(
    (url) =>
      /\/api\/agents\/composer-agent\/chats\/composer-chat$/.test(url.pathname),
    (route) =>
      route.fulfill({
        json: {
          status: "idle",
          model: { provider_id: "test", model: "test" },
          messages: [
            {
              id: "wrapping-user",
              role: "user",
              content:
                "A message that fits the wide view but wraps into several lines on a narrow phone. ".repeat(
                  2,
                ),
            },
          ],
        },
      }),
  );
  await page.reload();
  await expect(
    page.locator('[data-rua-chat-message-id="wrapping-user"]'),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Collapse user input" }),
  ).toHaveCount(0);
  await page.setViewportSize({ width: 360, height: 640 });
  await page.getByRole("button", { name: "Collapse user input" }).click();
  await expect(
    page.getByRole("button", { name: "Expand user input" }),
  ).toBeVisible();
  await page.setViewportSize({ width: 1100, height: 844 });
  await expect(
    page.getByRole("button", { name: "Expand user input" }),
  ).toHaveCount(0);
});
