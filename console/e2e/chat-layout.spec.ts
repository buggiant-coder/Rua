import { expect, test, type Page } from "@playwright/test";

async function renderChatLayoutFixture(
  page: Page,
  mobile: boolean,
  backgroundWork = false,
) {
  await page.goto("/");
  await page.addStyleTag({ path: "src/pages/Chat/owned/owned-conversation-surface.css" });
  await page.addStyleTag({ path: "src/pages/Chat/owned/owned-conversation-composer.css" });
  await page.addStyleTag({ path: "src/pages/Chat/owned/conversation-message-list.css" });
  await page.evaluate(
    ({ mobileMode, backgroundWork }) => {
      document.documentElement.classList.toggle("mobile-mode", mobileMode);
      document.getElementById("root")!.innerHTML = `
      <section class="rua-owned-surface" style="height: 600px">
        <div
          class="rua-owned-surface-messages"
          style="scrollbar-gutter: stable"
        >
          <div class="rua-chat-message-list">
            <article class="rua-chat-message rua-chat-message--assistant">
              <div
                class="rua-chat-bubble rua-chat-bubble--assistant"
                style="height: 720px"
              ></div>
              <div class="rua-chat-response-footer-row"></div>
            </article>
          </div>
        </div>
        ${
          backgroundWork
            ? '<div class="rua-owned-background-work"><div class="rua-owned-background-work-row">后台任务运行中（1）</div></div>'
            : ""
        }
        <div class="rua-owned-surface-composer">
          <div class="rua-owned-composer"></div>
        </div>
      </section>
    `;

      const scroller = document.querySelector<HTMLElement>(
        ".rua-owned-surface-messages",
      );
      if (scroller) scroller.scrollTop = scroller.scrollHeight;
    },
    { mobileMode: mobile, backgroundWork },
  );
}

async function chatLayoutMetrics(page: Page) {
  return page.evaluate(() => {
    const bubble = document.querySelector<HTMLElement>(
      ".rua-chat-bubble--assistant",
    )!;
    const footer = document.querySelector<HTMLElement>(
      ".rua-chat-response-footer-row",
    )!;
    const composer = document.querySelector<HTMLElement>(
      ".rua-owned-composer",
    )!;

    const bubbleRect = bubble.getBoundingClientRect();
    const footerRect = footer.getBoundingClientRect();
    const composerRect = composer.getBoundingClientRect();

    return {
      rightEdgeDelta: composerRect.right - bubbleRect.right,
      footerToComposerGap: composerRect.top - footerRect.bottom,
    };
  });
}

for (const viewport of [
  { name: "desktop", width: 1440, height: 900, mobile: false },
  { name: "mobile", width: 850, height: 900, mobile: true },
]) {
  test(`${viewport.name} aligns chat content and composer`, async ({
    page,
  }) => {
    await page.setViewportSize(viewport);
    await renderChatLayoutFixture(page, viewport.mobile);

    const metrics = await chatLayoutMetrics(page);
    expect(Math.abs(metrics.rightEdgeDelta)).toBeLessThan(0.5);
    expect(metrics.footerToComposerGap).toBeGreaterThanOrEqual(5);
    expect(metrics.footerToComposerGap).toBeLessThan(8);
  });

  test(`${viewport.name} aligns background work with composer and keeps a compact gap`, async ({
    page,
  }) => {
    await page.setViewportSize(viewport);
    await renderChatLayoutFixture(page, viewport.mobile, true);

    const metrics = await page.evaluate(() => {
      const status = document
        .querySelector(".rua-owned-background-work")!
        .getBoundingClientRect();
      const composer = document
        .querySelector(".rua-owned-composer")!
        .getBoundingClientRect();
      return {
        leftEdgeDelta: status.left - composer.left,
        rightEdgeDelta: status.right - composer.right,
        gap: composer.top - status.bottom,
      };
    });

    expect(Math.abs(metrics.leftEdgeDelta)).toBeLessThan(0.5);
    expect(Math.abs(metrics.rightEdgeDelta)).toBeLessThan(0.5);
    expect(metrics.gap).toBe(6);
  });
}
