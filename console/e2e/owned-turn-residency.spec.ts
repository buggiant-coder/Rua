import { expect, test } from "@playwright/test";

test.beforeEach(async ({ page }) => {
  await page.route(
    (url) => url.pathname.startsWith("/api/"),
    async (route) => {
      const url = new URL(route.request().url());
      const messages = Array.from({ length: 400 }, (_, index) => [
        {
          id: "user-" + index,
          turn_id: "turn-" + index,
          role: "user",
          content: "Question " + index,
        },
        {
          id: "answer-" + index,
          turn_id: "turn-" + index,
          role: "assistant",
          type: "message",
          content:
            "## Answer " +
            index +
            "\n\nA **formatted** answer.\n\n- First point\n- Second point\n\n" +
            "\x60\x60\x60js\nconst example = " +
            index +
            ";\n\x60\x60\x60",
        },
      ]).flat();
      let body: unknown = {};
      if (/\/chats\/[^/]+$/.test(url.pathname))
        body = { title: "Long history", status: "idle", messages };
      else if (url.pathname.endsWith("/commands")) body = [];
      else if (
        url.pathname.includes("approval") ||
        url.pathname.includes("permission")
      )
        body = { pending: [], resolved: [] };
      else if (url.pathname.includes("active-models"))
        body = { active_model: { provider_id: "test", model: "test" } };
      await route.fulfill({ json: body });
    },
  );
});

test("bounds mounted whole turns and restores both ends without scrolling gaps", async ({
  page,
}) => {
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.goto("/e2e/fixtures/owned-composer.html");
  const scroller = page.locator(".rua-owned-surface-messages");
  await expect(page.getByText("Answer 399", { exact: true })).toBeInViewport();
  await expect
    .poll(() => page.locator(".rua-chat-turn").count())
    .toBeLessThanOrEqual(256);
  await expect(page.locator("[data-rua-turn-gap]")).toHaveCount(1);
  await scroller.evaluate((node) => {
    node.scrollTop = 0;
  });
  await expect(page.getByText("Question 0", { exact: true })).toBeInViewport();
  await expect
    .poll(() => page.locator(".rua-chat-turn").count())
    .toBeLessThanOrEqual(256);
  await page.getByRole("button", { name: "到最下", exact: true }).click();
  await expect(page.getByText("Answer 399", { exact: true })).toBeInViewport();
  const incomplete = await page
    .locator(".rua-chat-turn")
    .evaluateAll(
      (turns) =>
        turns.filter(
          (turn) =>
            turn.querySelectorAll(".rua-chat-message--user").length !== 1 ||
            turn.querySelectorAll(".rua-chat-message--assistant").length !==
              1,
        ).length,
    );
  expect(incomplete).toBe(0);
  expect(errors).toEqual([]);
});

test("restores collapsed code and keeps the same reading position through repeated far jumps", async ({
  page,
}) => {
  await page.goto("/e2e/fixtures/owned-composer.html");
  const scroller = page.locator(".rua-owned-surface-messages");
  await expect(page.getByText("Answer 399", { exact: true })).toBeInViewport();
  const last = page.locator('[data-rua-turn-key="user-398"]');
  await scroller.hover();
  await page.mouse.wheel(0, -320);
  await last
    .getByRole("button", { name: "Collapse block", exact: true })
    .click();
  await expect(last.locator(".rua-render-block")).toHaveAttribute(
    "data-collapsed",
    "true",
  );
  await page.locator(".rua-owned-composer textarea").click();
  for (let index = 0; index < 3; index++) {
    await scroller.evaluate((node) => {
      node.scrollTop = 0;
    });
    await expect(
      page.getByText("Question 0", { exact: true }),
    ).toBeInViewport();
    if (index === 0) await expect(last).toHaveCount(0);
    await page.getByRole("button", { name: "到最下", exact: true }).click();
    await expect(
      page.getByText("Answer 399", { exact: true }),
    ).toBeInViewport();
    await expect(last.locator(".rua-render-block")).toHaveAttribute(
      "data-collapsed",
      "true",
    );
    await expect
      .poll(() => page.locator(".rua-chat-turn").count())
      .toBeLessThanOrEqual(256);
  }
});

test("search restores a reclaimed target by stable message identity", async ({
  page,
}) => {
  await page.goto("/e2e/fixtures/owned-composer.html");
  await expect(page.getByText("Answer 399", { exact: true })).toBeInViewport();
  await expect(page.locator('[data-rua-turn-key="user-4"]')).toHaveCount(0);
  await page.evaluate(() => {
    sessionStorage.setItem(
      "rua_search_scroll_target",
      JSON.stringify({
        ownerKey: "composer-agent",
        conversationId: "composer-chat",
        turnId: "turn-4",
        messageId: "answer-4",
        cardIndex: 99999,
        query: "Answer 4",
      }),
    );
    window.dispatchEvent(new Event("rua-search-scroll"));
  });
  await expect(page.getByText("Answer 4", { exact: true })).toBeInViewport();
  await expect(page.locator(".rua-search-jump-mark").first()).toBeVisible();
});

test("keeps ordinary history mounted and bounds continual growth after crossing the budget", async ({
  page,
}) => {
  await page.goto("/e2e/fixtures/turn-residency.html?_mobile=1");
  await expect(page.locator(".rua-chat-turn")).toHaveCount(180);
  const tail = page.locator('[data-rua-turn-key="user-199"]');
  await expect(tail).toBeVisible();
  await tail.evaluate((node) => {
    (window as Window & { retainedNode?: Element }).retainedNode = node;
  });
  for (let n = 0; n < 40; n++)
    await page
      .getByRole("button", { name: "Append turn", exact: true })
      .click();
  await expect(page.getByText("Answer 239", { exact: true })).toBeInViewport();
  await expect
    .poll(() => page.locator(".rua-chat-turn").count())
    .toBeLessThanOrEqual(192);
  expect(
    await tail.evaluate(
      (node) =>
        node === (window as Window & { retainedNode?: Element }).retainedNode,
    ),
  ).toBe(true);
  await expect(page.locator("[data-rua-turn-gap]")).toHaveCount(1);
});

test("prepend and live text retain the reading anchor in the middle", async ({
  page,
}) => {
  await page.goto("/e2e/fixtures/turn-residency.html?_mobile=1");
  const scroller = page.locator(".rua-owned-surface-messages");
  await expect(page.getByText("Answer 199", { exact: true })).toBeInViewport();
  await scroller.evaluate((node) => {
    node.scrollTop = node.scrollHeight * 0.55;
  });
  const anchor = await page.locator(".rua-chat-turn").evaluateAll((turns) => {
    const top = document
      .querySelector(".rua-owned-surface-messages")!
      .getBoundingClientRect().top;
    const row = turns.find(
      (row) => row.getBoundingClientRect().bottom > top + 1,
    )!;
    return {
      key: row.getAttribute("data-rua-turn-key"),
      offset: row.getBoundingClientRect().top - top,
    };
  });
  await page.getByRole("button", { name: "Prepend page", exact: true }).click();
  const target = page.locator('[data-rua-turn-key="' + anchor.key + '"]');
  const offset = () =>
    target.evaluate(
      (row) =>
        row.getBoundingClientRect().top -
        document
          .querySelector(".rua-owned-surface-messages")!
          .getBoundingClientRect().top,
    );
  await expect
    .poll(async () => Math.abs((await offset()) - anchor.offset))
    .toBeLessThan(3);
  for (let n = 0; n < 6; n++)
    await page
      .getByRole("button", { name: "Stream update", exact: true })
      .click();
  await expect
    .poll(async () => Math.abs((await offset()) - anchor.offset))
    .toBeLessThan(3);
});

test("keeps the visible turn stable through width changes and fast reversals", async ({
  page,
}) => {
  await page.goto("/e2e/fixtures/owned-composer.html?_mobile=1");
  const scroller = page.locator(".rua-owned-surface-messages");
  await expect(page.getByText("Answer 399", { exact: true })).toBeInViewport();
  await scroller.evaluate((node) => {
    node.scrollTop = node.scrollHeight * 0.45;
  });
  await expect
    .poll(() =>
      scroller.evaluate((node) => {
        const top = node.getBoundingClientRect().top;
        return [...node.querySelectorAll(".rua-chat-message")].some(
          (row) =>
            row.getBoundingClientRect().top < top + node.clientHeight &&
            row.getBoundingClientRect().bottom > top + 1,
        );
      }),
    )
    .toBe(true);
  const anchor = await scroller.evaluate((node) => {
    const top = node.getBoundingClientRect().top;
    const message = [
      ...node.querySelectorAll<HTMLElement>("[data-rua-chat-message-id]"),
    ].find((row) => row.getBoundingClientRect().bottom > top + 1)!;
    return {
      id: message.dataset.ruaChatMessageId,
      offset: message.getBoundingClientRect().top - top,
    };
  });
  await page.setViewportSize({ width: 500, height: 844 });
  const target = page.locator(
    '[data-rua-chat-message-id="' + anchor.id + '"]',
  );
  await expect(target).toBeAttached();
  await expect
    .poll(() =>
      target.evaluate(
        (node, offset) =>
          Math.abs(
            node.getBoundingClientRect().top -
              document
                .querySelector(".rua-owned-surface-messages")!
                .getBoundingClientRect().top -
              offset,
          ),
        anchor.offset,
      ),
    )
    .toBeLessThan(4);
  await scroller.hover();
  for (const amount of [-1800, 1800, -2400, 2400, -1000, 1000]) {
    await page.mouse.wheel(0, amount);
    await expect
      .poll(() =>
        scroller.evaluate((node) => {
          const rect = node.getBoundingClientRect();
          const element = document.elementFromPoint(
            rect.left + rect.width / 2,
            rect.top + rect.height / 2,
          );
          return Boolean(element?.closest(".rua-chat-turn"));
        }),
      )
      .toBe(true);
  }
  await expect
    .poll(() => page.locator(".rua-chat-turn").count())
    .toBeLessThanOrEqual(192);
});

test("protects selected history until the selection is cleared", async ({
  page,
}) => {
  await page.goto("/e2e/fixtures/owned-composer.html?_mobile=1");
  await expect(page.getByText("Answer 399", { exact: true })).toBeInViewport();
  const selected = page.locator('[data-rua-turn-key="user-398"]');
  await selected.evaluate((row) => {
    const range = document.createRange();
    range.selectNodeContents(row);
    const selection = window.getSelection()!;
    selection.removeAllRanges();
    selection.addRange(range);
  });
  const scroller = page.locator(".rua-owned-surface-messages");
  await scroller.evaluate((node) => {
    node.scrollTop = 0;
  });
  await expect(page.getByText("Question 0", { exact: true })).toBeInViewport();
  await expect(selected).toHaveCount(1);
  expect(
    await page.evaluate(() =>
      window.getSelection()!.toString().includes("Answer 398"),
    ),
  ).toBe(true);
  await page.evaluate(() => window.getSelection()!.removeAllRanges());
});

test("locates an unloaded turn and fills the visible history gap through bounded reads", async ({
  page,
}) => {
  const reads: string[] = [];
  await page.route(
    (url) => /\/chats\/[^/]+$/.test(url.pathname),
    async (route) => {
      const url = new URL(route.request().url());
      reads.push(url.search);
      const [first, last] = url.searchParams.has("around_turn_id")
        ? [1, 10]
        : url.searchParams.has("before_turn_ordinal")
        ? [1, 20]
        : [21, 40];
      await route.fulfill({
        json: {
          title: "Windowed history",
          oldest_turn_ordinal: first,
          newest_turn_ordinal: last,
          has_more_before: first > 1,
          messages: Array.from({ length: last - first + 1 }, (_, i) => [
            {
              id: "u" + (first + i),
              turn_id: "t" + (first + i),
              role: "user",
              content: "Question " + (first + i),
            },
            {
              id: "a" + (first + i),
              turn_id: "t" + (first + i),
              role: "assistant",
              content: "Answer " + (first + i),
            },
          ]).flat(),
        },
      });
    },
  );
  await page.goto("/e2e/fixtures/owned-composer.html");
  await expect(page.getByText("Answer 40", { exact: true })).toBeInViewport();
  await page.evaluate(() => {
    sessionStorage.setItem(
      "rua_search_scroll_target",
      JSON.stringify({
        conversationId: "composer-chat",
        ownerKey: "composer-agent",
        messageId: "a5",
        turnId: "t5",
        query: "Answer 5",
      }),
    );
    window.dispatchEvent(new Event("rua-search-scroll"));
  });
  await expect(page.getByText("Answer 5", { exact: true })).toBeInViewport();
  expect(
    reads.some(
      (query) =>
        query.includes("around_turn_id=t5") && query.includes("turn_limit=20"),
    ),
  ).toBe(true);
  const gap = page.locator(".rua-history-range-gap");
  await expect(gap).toHaveText("Load 10 intervening turns");
  await gap.click();
  await expect(gap).toHaveCount(0);
  await expect(page.getByText("Answer 11", { exact: true })).toBeAttached();
  await expect(page.getByText("Answer 40", { exact: true })).toBeAttached();
});

test("keeps mounted DOM bounded with thousands of retained data turns", async ({
  page,
}) => {
  await page.goto("/e2e/fixtures/turn-residency.html?_mobile=1&count=2000");
  await expect(page.getByText("Answer 2019", { exact: true })).toBeInViewport();
  await expect
    .poll(() => page.locator(".rua-chat-turn").count())
    .toBeLessThanOrEqual(192);
  await expect
    .poll(() => page.locator(".rua-chat-message").count())
    .toBeLessThanOrEqual(384);
  await page.getByRole("button", { name: "First", exact: true }).click();
  await expect(page.getByText("Question 20", { exact: true })).toBeInViewport();
  await expect
    .poll(() => page.locator(".rua-chat-turn").count())
    .toBeLessThanOrEqual(192);
});

test("keeps the budget while touch scrolling continues without an idle pause", async ({
  page,
}) => {
  await page.goto("/e2e/fixtures/turn-residency.html?_mobile=1&count=2000");
  await expect(page.getByText("Answer 2019", { exact: true })).toBeInViewport();
  const counts = await page
    .locator(".rua-owned-surface-messages")
    .evaluate(async (node) => {
      const counts: number[] = [];
      for (let n = 0; n < 90; n++) {
        node.dispatchEvent(new Event("touchmove", { bubbles: true }));
        node.scrollTop =
          (node.scrollHeight - node.clientHeight) * (1 - (n + 1) / 100);
        await new Promise((resolve) => requestAnimationFrame(resolve));
        counts.push(node.querySelectorAll(".rua-chat-turn").length);
      }
      node.dispatchEvent(new Event("touchend", { bubbles: true }));
      return counts;
    });
  expect(Math.max(...counts)).toBeLessThanOrEqual(200);
  await expect
    .poll(() => page.locator(".rua-chat-turn").count())
    .toBeLessThanOrEqual(192);
});

test("preserves the reader when an earlier mounted turn grows after layout", async ({
  page,
}) => {
  await page.goto("/e2e/fixtures/turn-residency.html?_mobile=1");
  const scroller = page.locator(".rua-owned-surface-messages");
  await expect(page.getByText("Answer 199", { exact: true })).toBeInViewport();
  await scroller.evaluate((node) => {
    node.scrollTop = node.scrollHeight * 0.7;
  });
  const anchor = await scroller.evaluate((node) => {
    const top = node.getBoundingClientRect().top;
    const row = [
      ...node.querySelectorAll<HTMLElement>("[data-rua-chat-message-id]"),
    ].find((row) => row.getBoundingClientRect().bottom > top + 1)!;
    return {
      id: row.dataset.ruaChatMessageId,
      offset: row.getBoundingClientRect().top - top,
    };
  });
  await page.locator('[data-rua-turn-key="user-30"]').evaluate((row) => {
    const media = document.createElement("div");
    media.style.height = "240px";
    media.setAttribute("data-test-delayed-media", "");
    row.appendChild(media);
  });
  const target = page.locator(
    '[data-rua-chat-message-id="' + anchor.id + '"]',
  );
  await expect
    .poll(() =>
      target.evaluate(
        (row, offset) =>
          Math.abs(
            row.getBoundingClientRect().top -
              document
                .querySelector(".rua-owned-surface-messages")!
                .getBoundingClientRect().top -
              offset,
          ),
        anchor.offset,
      ),
    )
    .toBeLessThan(3);
});

test("preserves the reading anchor when keyboard and rotation change viewport height", async ({
  page,
}) => {
  await page.goto("/e2e/fixtures/turn-residency.html?_mobile=1&count=2000");
  await expect(page.getByText("Answer 2019", { exact: true })).toBeInViewport();
  const scroller = page.locator(".rua-owned-surface-messages");
  await scroller.evaluate((node) => {
    node.scrollTop = node.scrollHeight * 0.5;
  });
  await expect
    .poll(() =>
      scroller.evaluate((node) => {
        const rect = node.getBoundingClientRect();
        return Boolean(
          document
            .elementFromPoint(rect.width / 2, rect.top + 10)
            ?.closest(".rua-chat-turn"),
        );
      }),
    )
    .toBe(true);
  const anchor = await scroller.evaluate((node) => {
    const top = node.getBoundingClientRect().top;
    const message = [
      ...node.querySelectorAll<HTMLElement>("[data-rua-chat-message-id]"),
    ].find((row) => row.getBoundingClientRect().bottom > top + 1)!;
    return {
      id: message.dataset.ruaChatMessageId,
      offset: message.getBoundingClientRect().top - top,
    };
  });
  const target = page.locator(
    '[data-rua-chat-message-id="' + anchor.id + '"]',
  );
  for (const viewport of [
    { width: 390, height: 530 },
    { width: 844, height: 390 },
    { width: 390, height: 844 },
  ]) {
    await page.setViewportSize(viewport);
    await expect
      .poll(() =>
        target.evaluate(
          (row, offset) =>
            Math.abs(
              row.getBoundingClientRect().top -
                document
                  .querySelector(".rua-owned-surface-messages")!
                  .getBoundingClientRect().top -
                offset,
            ),
          anchor.offset,
        ),
      )
      .toBeLessThan(4);
  }
});
