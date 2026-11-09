import {
  test,
  expect,
  goChat,
  submitPrompt,
  navigateSidebar,
  waitForChatUrl,
  captureChatStreamRequest,
  captureChatDeleteRequest,
  DEFAULT_PROMPT,
} from "./fixtures";

function locateDrawerTrigger(page: import("@playwright/test").Page) {
  return page
    .locator(
      [
        '[data-testid="chat-history-trigger"]',
        'button[aria-label*="history" i]',
        'button[aria-label*="历史"]',
        'button[aria-label*="会话"]',
        'button[aria-label*="全部聊天"]',
        'button[aria-label*="chat list" i]',
        'button:has-text("历史")',
        'button:has-text("全部聊天")',
      ].join(", "),
    )
    .first();
}

async function attachConsole(
  testInfoLike: {
    attach: (
      name: string,
      opts: { body: string; contentType: string },
    ) => Promise<void>;
  },
  sink: { messages: string[] },
  name: string,
): Promise<void> {
  const blob = sink.messages.join("\n");
  await testInfoLike.attach(name, {
    body: blob.length ? blob : "(no browser console output captured)",
    contentType: "text/plain",
  });
}

test("bug1 — /mcp → /chat round-trip during streaming leaves wrong chat in URL", async ({
  page,
  consoleLogSink,
}) => {
  void consoleLogSink;

  await goChat(page);

  const reqPromise = captureChatStreamRequest(page);

  await submitPrompt(page, DEFAULT_PROMPT);

  let postedSessionId: string | null = null;
  try {
    const req = await reqPromise;
    try {
      const body = req.postDataJSON() as { session_id?: string };
      postedSessionId = body?.session_id ?? null;
    } catch {
      postedSessionId = null;
    }
  } catch {}

  let chatIdAfterFirstFrame: string;
  try {
    chatIdAfterFirstFrame = await waitForChatUrl(page);
  } catch (e) {
    test.info().annotations.push({
      type: "diag-note",
      description: `URL never became /chat/<id> after submit. posted_session_id=${postedSessionId}. err=${
        (e as Error).message
      }`,
    });
    throw e;
  }

  await navigateSidebar(page, "mcp");
  await page.waitForTimeout(1_500);
  await navigateSidebar(page, "chat");
  await page.waitForTimeout(1_500);

  const url = page.url();
  const matched = url.match(/\/chat\/([^/?#]+)/)?.[1];

  test.info().annotations.push({
    type: "diag-bug1-evidence",
    description: JSON.stringify({
      posted_session_id: postedSessionId,
      first_frame_chat_id: chatIdAfterFirstFrame,
      url_after_round_trip: url,
      url_chat_id_after_round_trip: matched ?? null,
    }),
  });

  await attachConsole(test.info(), consoleLogSink, "bug1-browser-console.log");

  expect(
    matched,
    `bug1: URL should stay on /chat/${chatIdAfterFirstFrame} but got ${url}`,
  ).toBe(chatIdAfterFirstFrame);
});

test("bug2 — drawer trigger button (NOT list click) snaps URL back to streaming chat", async ({
  page,
  consoleLogSink,
}) => {
  void consoleLogSink;

  await goChat(page);
  const reqPromise = captureChatStreamRequest(page);
  await submitPrompt(page, DEFAULT_PROMPT);
  await reqPromise.catch(() => {});

  const chatA = await waitForChatUrl(page);

  await navigateSidebar(page, "mcp");
  await page.waitForTimeout(1_500);
  await navigateSidebar(page, "chat");
  await page.waitForTimeout(1_500);

  const urlAfterRoundTrip = page.url();
  const idAfterRoundTrip =
    urlAfterRoundTrip.match(/\/chat\/([^/?#]+)/)?.[1] ?? null;

  test.info().annotations.push({
    type: "diag-bug2-precondition",
    description: JSON.stringify({
      chat_a_streaming: chatA,
      url_after_mcp_chat_round_trip: urlAfterRoundTrip,
      id_after_round_trip: idAfterRoundTrip,
      bug1_reproduced: idAfterRoundTrip !== null && idAfterRoundTrip !== chatA,
    }),
  });

  if (!idAfterRoundTrip || idAfterRoundTrip === chatA) {
    test.info().annotations.push({
      type: "diag-bug2-skip-no-bug1",
      description:
        "bug1 did not snap URL to a different chat — cannot exercise bug2.",
    });
    await attachConsole(
      test.info(),
      consoleLogSink,
      "bug2-browser-console.log",
    );
    return;
  }
  const chatB = idAfterRoundTrip;

  const trigger = locateDrawerTrigger(page);
  const triggerCount = await trigger.count();
  if (!triggerCount) {
    test.info().annotations.push({
      type: "diag-bug2-skip-no-trigger",
      description:
        "drawer trigger button not found by aria/text selectors — DOM probably changed",
    });
    await attachConsole(
      test.info(),
      consoleLogSink,
      "bug2-browser-console.log",
    );
    return;
  }
  await trigger.click().catch(() => {});

  await page.waitForTimeout(500);

  const urlAfterTriggerClick = page.url();
  const idAfterTrigger =
    urlAfterTriggerClick.match(/\/chat\/([^/?#]+)/)?.[1] ?? null;

  test.info().annotations.push({
    type: "diag-bug2-evidence",
    description: JSON.stringify({
      chat_a_streaming: chatA,
      chat_b_url_before_drawer_open: chatB,
      url_after_trigger_click: urlAfterTriggerClick,
      id_after_trigger: idAfterTrigger,
      bug2_repro: idAfterTrigger === chatA,
    }),
  });

  await attachConsole(test.info(), consoleLogSink, "bug2-browser-console.log");

  expect(
    idAfterTrigger,
    `bug2: drawer-trigger click should leave URL on /chat/${chatB} but got ${urlAfterTriggerClick}`,
  ).toBe(chatB);
});

test("bug3 — switching INTO an older session via drawer while A streams shows blank webui", async ({
  page,
  consoleLogSink,
}) => {
  void consoleLogSink;

  await goChat(page);
  await submitPrompt(page, DEFAULT_PROMPT);
  const chatA = await waitForChatUrl(page);
  await page.waitForTimeout(2_000);

  const trigger = locateDrawerTrigger(page);
  const triggerCount = await trigger.count();
  if (!triggerCount) {
    test.info().annotations.push({
      type: "diag-bug3-skip-no-trigger",
      description: "drawer trigger button not found",
    });
    await attachConsole(
      test.info(),
      consoleLogSink,
      "bug3-browser-console.log",
    );
    return;
  }
  await trigger.click().catch(() => {});
  await page.waitForTimeout(800);

  const items = page.locator(
    [
      "[data-session-id]",
      '[class*="sessionItem"]',
      '[class*="SessionItem"]',
      '.ant-drawer-body [role="listitem"]',
    ].join(", "),
  );
  const total = await items.count();
  let switchedToId: string | null = null;
  for (let i = 0; i < total; i++) {
    const item = items.nth(i);
    const id = await item.getAttribute("data-session-id");
    if (id && id !== chatA) {
      switchedToId = id;
      await item.click().catch(() => {});
      break;
    }
  }

  if (!switchedToId) {
    test.info().annotations.push({
      type: "diag-bug3-skip-no-other-session",
      description:
        "drawer had no older session to switch to — needs a second pre-existing chat",
    });
    await attachConsole(
      test.info(),
      consoleLogSink,
      "bug3-browser-console.log",
    );
    return;
  }

  await page.waitForTimeout(2_000);

  const messages = page.locator(
    [
      "[data-message-role]",
      '[class*="messageBubble"]',
      '[class*="message-bubble"]',
      '[class*="MessageBubble"]',
      ".rua-bubble",
    ].join(", "),
  );
  const visibleMessages = await messages.count();
  const urlNow = page.url();
  const idNow = urlNow.match(/\/chat\/([^/?#]+)/)?.[1] ?? null;

  test.info().annotations.push({
    type: "diag-bug3-evidence",
    description: JSON.stringify({
      chat_a_streaming: chatA,
      switched_to_b: switchedToId,
      url_after_switch: urlNow,
      id_after_switch: idNow,
      message_bubble_count_in_b: visibleMessages,
    }),
  });

  await attachConsole(test.info(), consoleLogSink, "bug3-browser-console.log");

  expect(
    visibleMessages,
    `bug3: after switching from streaming chatA=${chatA} into chatB=${switchedToId}, expected B's persisted messages to render (>=1 bubble) but found ${visibleMessages}`,
  ).toBeGreaterThan(0);
});

test("bug4 — deleting chat on /chats then reloading restores it (server-side persistence)", async ({
  page,
}) => {
  await goChat(page);
  await submitPrompt(page, DEFAULT_PROMPT);
  const newChatId = await waitForChatUrl(page);
  await page.waitForTimeout(1_500);

  for (const path of ["/chats", "/chat-history", "/history"]) {
    await page.goto(path).catch(() => {});
    if (page.url().includes(path)) break;
  }
  await page.waitForTimeout(1_000);

  const delBtn = page
    .locator(
      'button[aria-label*="delete"], button[aria-label*="删除"], button:has-text("删除"), [data-testid="chat-delete"]',
    )
    .first();
  if (!(await delBtn.count())) {
    test.info().annotations.push({
      type: "diag-bug4-skip",
      description:
        "no delete button found on chats page — skipping. URL=" + page.url(),
    });
    return;
  }

  const delPromise = captureChatDeleteRequest(page).catch(() => null);
  await delBtn.click().catch(() => {});

  const confirm = page
    .locator(
      'button:has-text("确定"), button:has-text("Yes"), button:has-text("OK")',
    )
    .first();
  if (await confirm.count()) {
    await confirm.click().catch(() => {});
  }

  const delReq = await delPromise;
  const delUrl = delReq?.url() ?? null;
  await page.waitForTimeout(1_500);

  const before = await page
    .locator(
      '[class*="chatItem"], [class*="ChatItem"], tr[data-row-key], li[data-session-id]',
    )
    .count();

  await page.reload({ waitUntil: "domcontentloaded" });
  await page.waitForTimeout(1_500);

  const after = await page
    .locator(
      '[class*="chatItem"], [class*="ChatItem"], tr[data-row-key], li[data-session-id]',
    )
    .count();

  test.info().annotations.push({
    type: "diag-bug4-evidence",
    description: JSON.stringify({
      new_chat_id: newChatId,
      delete_request_url: delUrl,
      list_count_pre_reload: before,
      list_count_post_reload: after,
    }),
  });

  expect(
    after,
    `bug4: chat list should not grow after reload (pre=${before}, post=${after}) — server did not honor DELETE`,
  ).toBeLessThanOrEqual(before);
});
